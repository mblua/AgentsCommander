#[tokio::test]
async fn issue_3145_cold_duplicate_recipes_block_before_first_restore_publication() {
    use tauri::Listener;
    for fresh in [false, true] {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_text = cwd.path().to_string_lossy().into_owned();
        let settings = test_settings();
        let spawn =
            super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd_text, None)
                .unwrap()
                .unwrap();
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(settings, sessions.clone(), pty.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let recipe = crate::config::sessions_persistence::PersistedSession {
            name: "first duplicate".into(),
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            working_directory: cwd_text.clone(),
            agent_id: Some("codex".into()),
            requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
            effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
            start_fresh_on_restore: fresh,
            ..Default::default()
        };
        let manager = sessions.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
        let key = crate::config::sessions_persistence::prompt_key(
            &cwd_text,
            recipe.agent_id.as_deref(),
            recipe.requested_profile.as_deref(),
            recipe.effective_profile.as_deref(),
        )
        .unwrap();
        manager
            .install_retained_prompts(Some(HashMap::from([(
                key.clone(),
                crate::session::manager::RetainedPrompt {
                    last_prompt: Some("unproven old cache".into()),
                    blocked: false,
                },
            )])))
            .await;
        let mut dormant_competitor = recipe.clone();
        dormant_competitor.name = "second dormant duplicate".into();
        dormant_competitor.status = Some(SessionStatus::Exited(0));
        crate::config::sessions_persistence::block_duplicate_restore_prompt_domains(
            &manager,
            &[recipe.clone(), dormant_competitor],
        )
        .await;
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured = events.clone();
        app.listen("session_created", move |event| {
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        let result = super::create_session_inner_for_restore(
            &transaction,
            &sessions,
            &pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd_text,
            Some(recipe.name.clone()),
            Some("codex".into()),
            None,
            true,
            Vec::new(),
            fresh,
            Some(spawn),
            None,
            None,
            Some(fresh),
            None,
            None,
            Some(&recipe),
        )
        .await
        .unwrap();
        assert_eq!(result.last_prompt, None);
        assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert!(events[0]["lastPrompt"].is_null());
        drop(transaction);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn issue_3145_wake_and_dormant_publish_initial_prompt_before_observation() {
    use tauri::Listener;
    for (dormant, root) in [(false, false), (false, true), (true, false), (true, true)] {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_text = cwd.path().to_string_lossy().into_owned();
        let settings = test_settings();
        let spawn =
            super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd_text, None)
                .unwrap()
                .unwrap();
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(settings, sessions.clone(), pty.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        if root {
            super::coordinator_create_seam::force_root(&cwd_text);
        }
        let recipe = crate::config::sessions_persistence::PersistedSession {
            name: "restore prompt".into(),
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            working_directory: cwd_text.clone(),
            agent_id: Some("codex".into()),
            requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
            effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
            last_prompt: Some("A".into()),
            ..Default::default()
        };
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured = events.clone();
        app.listen("session_created", move |event| {
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        let info = if dormant {
            transaction
                .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                    persisted: recipe.clone(),
                    working_directory: cwd_text,
                    is_coordinator: false,
                    is_root_agent: root,
                })
                .await
                .unwrap()
        } else {
            super::create_session_inner_for_restore(
                &transaction,
                &sessions,
                &pty,
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                cwd_text,
                Some(recipe.name.clone()),
                Some("codex".into()),
                None,
                true,
                Vec::new(),
                false,
                Some(spawn),
                None,
                None,
                Some(false),
                None,
                None,
                Some(&recipe),
            )
            .await
            .unwrap()
        };
        assert_eq!(info.last_prompt.as_deref(), Some("A"));
        assert_eq!(
            sessions.read().await.list_sessions().await[0]
                .last_prompt
                .as_deref(),
            Some("A")
        );
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["lastPrompt"], "A");
        assert_eq!(info.is_root_agent, root);
        super::coordinator_create_seam::unforce_root(&recipe.working_directory);
        drop(transaction);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn issue_3145_reused_root_live_and_dormant_seed_event_never_overwrite_new_input() {
    use tauri::Listener;
    for dormant in [false, true] {
        for newer in [None, Some("N"), Some("")] {
            let cwd = tempfile::tempdir().unwrap();
            let cwd_text = cwd.path().to_string_lossy().into_owned();
            let settings = test_settings();
            let spawn =
                super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd_text, None)
                    .unwrap()
                    .unwrap();
            let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
            let backend = Arc::new(ScriptedSpawnBackend::default());
            let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
                backend,
            )));
            let app = session_test_app(settings, sessions.clone(), pty.clone());
            let transaction =
                crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
            let mut recipe = crate::config::sessions_persistence::PersistedSession {
                name: "reused root".into(),
                shell: spawn.shell.clone(),
                shell_args: spawn.shell_args.clone(),
                working_directory: cwd_text.clone(),
                agent_id: Some("codex".into()),
                requested_profile: Some(spawn.profile_resolution.requested_profile.clone()),
                effective_profile: Some(spawn.profile_resolution.effective_profile.clone()),
                ..Default::default()
            };
            let info = if dormant {
                transaction
                    .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                        persisted: recipe.clone(),
                        working_directory: cwd_text,
                        is_coordinator: false,
                        is_root_agent: true,
                    })
                    .await
                    .unwrap()
            } else {
                super::create_session_inner_for_restore(
                    &transaction,
                    &sessions,
                    &pty,
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    cwd_text,
                    Some(recipe.name.clone()),
                    Some("codex".into()),
                    None,
                    true,
                    Vec::new(),
                    false,
                    Some(spawn),
                    None,
                    None,
                    Some(false),
                    None,
                    None,
                    Some(&recipe),
                )
                .await
                .unwrap()
            };
            let id = Uuid::parse_str(&info.id).unwrap();
            let manager = sessions.read().await.clone();
            manager.set_is_root_agent(id, true).await;
            if let Some(text) = newer {
                manager.set_last_prompt(id, text.into()).await;
            }
            let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
            let captured = events.clone();
            app.listen("last_prompt", move |event| {
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(event.payload()).unwrap());
            });
            recipe.last_prompt = Some("A".into());
            super::seed_root_last_prompt(app.handle(), &sessions, id, &recipe).await;
            assert_eq!(
                manager
                    .get_session(id)
                    .await
                    .unwrap()
                    .last_prompt
                    .as_deref(),
                Some(newer.unwrap_or("A"))
            );
            let observed = events.lock().unwrap().clone();
            assert_eq!(observed.len(), usize::from(newer.is_none()));
            if newer.is_none() {
                assert_eq!(observed[0]["text"], "A");
            }
            drop(transaction);
            close_test_coordinator(&app).await;
        }
    }
}

use super::{
    classify_existing_root, claude_projects_dir_for_config_dir,
    claude_resume_probe_target_for_kind, coding_agent_descriptor, compute_profile_outdated,
    container_path_context_for_cwd, count_working_members, effective_restart_requested_profile,
    execute_manual_coordinator_destroy, inject_codex_resume, inject_pi_resume,
    injected_claude_config_dir_for_copy, maybe_inject_pi_resume, partition_restart_resume_ready,
    pi_has_explicit_session_control, pi_is_non_conversation_invocation,
    prepare_last_coding_agent_write, resolve_actual_agent, resolve_agent_command,
    resolve_agent_from_shell, resolve_claude_projects_dir, resolve_launch_auto_self_clear,
    resolve_launch_idle_tuning, resolve_restart_selected_agent_id, resolve_root_agent_command,
    restart_resume_prompt_for, restart_resume_session_is_ready, resume_probe_target_for_config_dir,
    save_last_coding_agent, should_inject_continue, CreateSelectionIntent, ExistingRootAction,
    RestartResumeTarget,
};
use crate::config::coding_agents_catalog::CodingAgentDefinition;
use crate::config::settings::{AgentConfig, AppSettings, ProfileCellConfig};
use crate::pty::backend::{PtyBackend, SessionBackendKind};
use crate::pty::container_backend::container_child_env;
use crate::pty::container_credentials::ContainerCredentialPlan;
use crate::pty::container_paths::{ContainerPathMap, CLAUDE_CONFIG_DIR_KEY, WARNING_KIND_NO_VALUE};
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::{
    SessionCommunication, SessionCommunicationKind, SessionInfo, SessionStatus,
};
use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

fn create_admission_owner(
    tracker: crate::pty::manager::ResponseCloseState,
    target: &str,
    cwd: &std::path::Path,
) -> Arc<Mutex<super::OwnedCreateAdmission>> {
    Arc::new(Mutex::new(super::OwnedCreateAdmission {
        preheld_target: false,
        owned_replacement: None,
        replacement_pty: None,
        tracker,
        target: target.to_owned(),
        cwd: cwd.to_path_buf(),
        nonce: Uuid::new_v4(),
        binding: SessionManager::new().reserve_carrier_create_binding(),
        reservation: None,
        failed_foreign: false,
        owned_target: None,
        capacity: None,
        resource: None,
        blocked_bound: false,
        capacity_error: None,
    }))
}

#[tokio::test]
async fn create_foreign_owners_are_independent_after_first_waiter_loss() {
    let temp = tempfile::tempdir().unwrap();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    let manager = SessionManager::new();
    let target = format!("foreign-create:{}", Uuid::new_v4());
    let first = create_admission_owner(tracker.clone(), &target, temp.path());
    let first_nonce = first.lock().unwrap().nonce;
    let task_manager = manager.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            let permit = super::acquire_create_publication_region(&first, &task_manager)
                .await
                .expect("Foreign region");
            assert!(first.lock().unwrap().reservation.is_some());
            drop(permit);
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            drop(first);
            finished_tx.send(()).unwrap();
        })
        .await;
    });
    started_rx.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let second = create_admission_owner(tracker.clone(), &target, temp.path());
    let permit = super::try_acquire_create_publication_region(&second, &manager)
        .await
        .expect("second Foreign create advances while first accepted core remains retained");
    assert!(second.lock().unwrap().reservation.is_some());
    use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseCloseError};
    assert!(matches!(
        tracker.reserve_create_owner(
            &target,
            first_nonce,
            CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Err(ResponseCloseError::Conflict)
    ));
    drop(permit);
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    assert!(matches!(
        tracker.reserve_create_owner(
            &target,
            first_nonce,
            CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Ok(CreateAdmission::Reserved(_))
    ));
}

#[tokio::test]
async fn create_e2_uses_same_permanently_failed_tracker_and_actual_foreign_scope() {
    let temp = tempfile::tempdir().unwrap();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    tracker.fail_closed();
    let manager = SessionManager::new();
    let owner = create_admission_owner(tracker.clone(), "failed-foreign", temp.path());
    let permit = super::try_acquire_create_publication_region(&owner, &manager)
        .await
        .expect("actual ALL-scope Foreign can use E2 after exact tracker Unavailable");
    {
        let owner = owner.lock().unwrap();
        assert!(owner.tracker.same_state(&tracker));
        assert!(owner.failed_foreign);
        assert!(owner.reservation.is_none());
    }
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert!(!owner.lock().unwrap().reserve_current(
                crate::config::sessions_persistence::CreateCarrierScope::Waiting,
                current
            ));
            assert!(
                !owner.lock().unwrap().failed_foreign,
                "Waiting cannot retain previous E2 admission evidence"
            );
        })
        .await;
    tracker.apply_settings(true, 30, std::time::Instant::now());
    assert!(matches!(
        tracker.reserve_create_owner(
            "failed-foreign",
            Uuid::new_v4(),
            crate::pty::manager::CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    ));
    drop(permit);
}

#[tokio::test]
async fn create_protected_owner_is_exclusive_and_failed_tracker_cannot_use_e2() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("member");
    std::fs::create_dir(&cwd).unwrap();
    let manager = SessionManager::new();
    let row = manager
        .create_session(
            "bash".into(),
            vec![],
            cwd.to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    manager.set_start_fresh_on_restore(row.id, true).await;
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let version = manager
        .with_current_carrier_snapshot(&permit, |current| current.versions[&row.id])
        .await;
    drop(permit);
    let target = format!("protected-create:{}", Uuid::new_v4());
    let carrier = crate::config::sessions_persistence::persist_initial_carrier(
        &manager,
        temp.path().join("config"),
        vec![temp.path().to_string_lossy().into_owned()],
        crate::config::sessions_persistence::InitialCarrierIntent::new(
            target.clone(),
            row.id,
            row.token,
            version,
            std::fs::canonicalize(&cwd).unwrap(),
        ),
    )
    .await
    .unwrap()
    .expect("actual current carrier confirmed");
    let tracker = crate::pty::manager::ResponseCloseState::default();
    tracker.register_route(row.id, 1, std::time::Instant::now());
    let first = create_admission_owner(tracker.clone(), &target, &cwd);
    assert!(
        super::try_acquire_create_publication_region(&first, &manager)
            .await
            .is_none(),
        "Protected awaits owned Preparing before any create effect"
    );
    assert!(
        first.lock().unwrap().reservation.is_none(),
        "generic Protected wait takes no exclusive claim"
    );
    let second = create_admission_owner(tracker.clone(), &target, &cwd);
    assert!(
        super::try_acquire_create_publication_region(&second, &manager)
            .await
            .is_none()
    );
    assert!(
        second.lock().unwrap().reservation.is_none(),
        "second generic waiter also takes no exclusive claim"
    );
    #[cfg(target_os = "windows")]
    let quota_waiter = {
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::new());
        let config = crate::config::settings::AppSettings {
            resource_monitor_enabled: true,
            max_concurrent_agent_processes: 1,
            ..Default::default()
        };
        let settings = crate::config::settings::new_settings_state(config.clone());
        let resource = Arc::new(Mutex::new(super::CreateResourceAdmission {
            monitor: monitor.clone(),
            settings: settings.clone(),
            permit: None,
            needed: true,
            in_region: false,
            released: false,
        }));
        let third = create_admission_owner(tracker.clone(), &target, &cwd);
        third.lock().unwrap().resource = Some(resource.clone());
        let quota = monitor
            .try_reserve_agent_slot(super::ResourceLimits::from(&config))
            .unwrap()
            .expect("Windows quota really reserves a slot");
        let manager = manager.clone();
        let waiter = tokio::spawn(async move {
            let mut local = Some(quota);
            let result = super::with_create_resource_region(
                &third,
                &mut local,
                super::acquire_create_publication_region(&third, &manager),
            )
            .await;
            (result.map(drop), local)
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !resource.lock().unwrap().released {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("blocked create releases its actual quota");
        let replacement_quota = monitor
            .try_reserve_agent_slot(super::ResourceLimits::from(&config))
            .unwrap()
            .expect("protected owner can use the released quota");
        monitor.release_unregistered_permit(replacement_quota);
        settings.write().await.max_concurrent_agent_processes = 0;
        waiter
    };
    tracker.fail_closed();
    assert!(
        super::try_acquire_create_publication_region(&first, &manager)
            .await
            .is_none()
    );
    assert!(
        !first.lock().unwrap().failed_foreign,
        "Protected cannot use E2"
    );
    drop(carrier);
    #[cfg(target_os = "windows")]
    {
        let (result, quota) = tokio::time::timeout(std::time::Duration::from_secs(5), quota_waiter)
            .await
            .expect("unblocked create completes its real fresh quota probe")
            .unwrap();
        assert!(result
            .unwrap_err()
            .contains("Resource Monitor cap reached: 0/0"));
        assert!(
            quota.is_none(),
            "failed readmission does not fabricate a permit"
        );
    }
    let permit = super::try_acquire_create_publication_region(&second, &manager)
        .await
        .expect(
            "same failed tracker permits only freshly classified Foreign after key lifetime ends",
        );
    assert!(second.lock().unwrap().failed_foreign);
    drop(permit);
}

// Stage E (#1064) selection-intent mapping sentinel (plan section 10.4 item
// 20, acceptance item 42, 10.6): the internal-alert background create keeps
// `CreateSelectionIntent::Background`, which maps to the Background trusted
// intent (never downgraded); Suppress finalizes nothing.
#[test]
fn stage_e_create_selection_intent_maps_background_to_trusted_background() {
    use crate::session::selection::TrustedCreateIntent;
    assert!(matches!(
        CreateSelectionIntent::Background.trusted(),
        Some(TrustedCreateIntent::Background)
    ));
    assert!(matches!(
        CreateSelectionIntent::User.trusted(),
        Some(TrustedCreateIntent::User)
    ));
    assert!(
        CreateSelectionIntent::Suppress.trusted().is_none(),
        "a suppressed create never becomes a trusted finalizing create"
    );
}

#[test]
fn count_working_members_counts_live_and_busy_only() {
    // tuples are (live /*PTY-backed*/, waiting_for_input)
    assert_eq!(count_working_members(&[]), 0, "empty -> 0");
    assert_eq!(
        count_working_members(&[(true, true), (true, true)]),
        0,
        "all live but idle (waiting) -> 0"
    );
    assert_eq!(
        count_working_members(&[(true, false), (true, false), (true, false)]),
        3,
        "all live and busy -> N"
    );
    assert_eq!(
        count_working_members(&[(true, true), (true, false)]),
        1,
        "only the live+busy member counts"
    );
    assert_eq!(
        count_working_members(&[(false, false), (false, false)]),
        0,
        "busy but dead (no live PTY) -> 0"
    );
    assert_eq!(
        count_working_members(&[(true, false), (false, false), (true, true)]),
        1,
        "mixed: one live+busy, one dead+busy, one live+idle -> 1"
    );
}

fn physical_binding_test_settings(dir: &std::path::Path) -> AppSettings {
    let path = dir.join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    std::fs::write(&path, serde_json::to_vec(&test_settings()).unwrap()).unwrap();
    crate::config::settings::load_settings_from_path(&path)
}

#[tokio::test]
async fn derived_agent_id_binding_creation_restart_and_plain_shell() {
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let physical = tempfile::tempdir().unwrap();
    let app = session_test_app(
        physical_binding_test_settings(physical.path()),
        session_mgr.clone(),
        pty_mgr.clone(),
    );
    let mut births = Vec::new();
    for index in 0..2 {
        let work = cwd.path().join(format!("binding-{index}"));
        std::fs::create_dir(&work).unwrap();
        let info = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            work.to_string_lossy().to_string(),
            Some(format!("binding-{index}")),
            Some("claude".into()),
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
        .unwrap();
        assert_eq!(info.agent_id.as_deref(), Some("claude"));
        let id = Uuid::parse_str(&info.id).unwrap();
        let manager = session_mgr.read().await.clone();
        let binding = manager.coding_agent_binding(id).await.unwrap();
        assert_ne!(binding.id, info.id);
        assert_eq!(binding.seed.as_ref().unwrap()["originalAgentId"], "claude");
        births.push((info, binding));
    }
    assert_ne!(births[0].1.id, births[1].1.id);
    let manager = session_mgr.read().await.clone();
    let saved_before_failure =
        crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    let invalid_dir = cwd.path().join("file-not-a-directory");
    std::fs::write(&invalid_dir, b"preserve foreign bytes").unwrap();
    assert!(
        crate::config::sessions_persistence::save_sessions_to_dir_for_test(
            &invalid_dir,
            &saved_before_failure
        )
        .is_err()
    );
    let after_failure = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    let row = after_failure
        .iter()
        .find(|row| row.id.as_deref() == Some(births[0].0.id.as_str()))
        .unwrap();
    assert_eq!(row.coding_agent_binding_id.as_ref(), Some(&births[0].1.id));
    assert_eq!(
        std::fs::read(&invalid_dir).unwrap(),
        b"preserve foreign bytes"
    );
    let old_id = Uuid::parse_str(&births[0].0.id).unwrap();
    let settings = app.state::<crate::config::settings::SettingsState>();
    let restarted = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(false),
        true,
    )
    .await
    .unwrap();
    assert_ne!(restarted.id, births[0].0.id);
    let after = manager
        .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
        .await
        .unwrap();
    assert_eq!(after.id, births[0].1.id);
    assert_eq!(after.seed, births[0].1.seed);
    assert!(manager.coding_agent_binding(old_id).await.is_none());
    let plain =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    assert!(manager
        .coding_agent_binding(Uuid::parse_str(&plain.id).unwrap())
        .await
        .is_none());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_save_replace_create_uses_post_edit_revision() {
    let physical = tempfile::tempdir().unwrap();
    let path = physical
        .path()
        .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    let current = physical_binding_test_settings(physical.path());
    let before = crate::config::settings::bootstrap_binding_revision(&current, "claude").unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(ScriptedSpawnBackend::default()),
    )));
    let app = session_test_app(current, session_mgr.clone(), pty_mgr.clone());
    let settings = app.state::<crate::config::settings::SettingsState>();
    let mut births: Vec<(
        crate::session::session::SessionInfo,
        crate::session::manager::CodingAgentBinding,
    )> = Vec::new();
    for index in 0..2 {
        let mut draft = settings.read().await.clone();
        if index == 1 {
            draft.agents[0].command = "claude --different-execution".into();
            draft.agents[0]
                .envs
                .push(crate::config::settings::CodingAgentEnv {
                    key: "BINDING_REVISION_TEST".into(),
                    value: "edited".into(),
                    source: Default::default(),
                    enabled: true,
                });
        }
        let written =
            crate::config::settings::save_settings_to_path_preserving_project_paths(&draft, &path)
                .unwrap();
        assert!(written.coding_agent_identity_physical.is_some());
        let expected =
            crate::config::settings::bootstrap_binding_revision(&written, "claude").unwrap();
        let fresh = crate::config::settings::load_settings_from_path(&path);
        assert_eq!(
            expected,
            crate::config::settings::bootstrap_binding_revision(&fresh, "claude").unwrap()
        );
        if index == 1 {
            assert_ne!(
                expected,
                births[0].1.seed.as_ref().unwrap()["identityRevision"]
            );
        }
        *settings.write().await = written;
        let work = cwd.path().join(format!("post-save-{index}"));
        std::fs::create_dir(&work).unwrap();
        let info = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            work.to_string_lossy().to_string(),
            Some(format!("post-save-{index}")),
            Some("claude".into()),
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
        .unwrap();
        let manager = session_mgr.read().await.clone();
        let binding = manager
            .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
            .await
            .unwrap();
        assert_eq!(binding.seed.as_ref().unwrap()["identityRevision"], expected);
        births.push((info, binding));
    }
    // Save may materialize identity rows, so compare against its real output.
    assert_ne!(
        births[1].1.seed.as_ref().unwrap()["identityRevision"],
        before
    );
    let restarted = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        Uuid::parse_str(&births[0].0.id).unwrap(),
        None,
        None,
        Some(false),
        true,
    )
    .await
    .unwrap();
    let manager = session_mgr.read().await.clone();
    let after = manager
        .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
        .await
        .unwrap();
    assert_eq!(after.id, births[0].1.id);
    assert_eq!(after.seed, births[0].1.seed);
    assert_ne!(after.seed, births[1].1.seed);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_wake_restore_preserves_birth_seed_and_origin() {
    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let physical = tempfile::tempdir().unwrap();
    let app = session_test_app(
        physical_binding_test_settings(physical.path()),
        session_mgr.clone(),
        pty_mgr.clone(),
    );
    let transaction =
        crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let birth = crate::session::manager::CodingAgentBinding {
        id: Uuid::new_v4().to_string(),
        seed: Some(
            serde_json::json!({"identityRevision":format!("r1:0:{}", "a".repeat(64)),"originalAgentId":"claude"}),
        ),
        origin: Some(
            serde_json::json!({"sessionsPath":"C:/original-sessions.json","rawFileSHA256":"physical-original","originalOrdinal":5}),
        ),
    };
    let info = super::create_session_inner_for_restore(
        &transaction,
        &session_mgr,
        &pty_mgr,
        "claude".into(),
        Vec::new(),
        cwd.path().to_string_lossy().to_string(),
        Some("wake restore".into()),
        Some("claude".into()),
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        Some(false),
        None,
        Some(birth.clone()),
        None,
    )
    .await
    .unwrap();
    let manager = session_mgr.read().await.clone();
    assert_eq!(info.agent_id.as_deref(), Some("claude"));
    let restored = manager
        .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
        .await
        .unwrap();
    assert_ne!(birth.id, info.id);
    assert_eq!(restored.id, birth.id);
    assert_eq!(restored.seed, birth.seed);
    assert_eq!(restored.origin, birth.origin);
    let snapshot = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
    assert_eq!(
        snapshot[0].coding_agent_binding_id.as_ref(),
        Some(&birth.id)
    );
    drop(transaction);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn derived_agent_id_binding_seedless_restore_and_new_birth_preserve_compatibility() {
    for case in ["removed", "ambiguous", "unreadable", "missing-physical"] {
        let physical = tempfile::tempdir().unwrap();
        let path = physical
            .path()
            .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
        let mut raw = serde_json::to_value(test_settings()).unwrap();
        if case == "removed" {
            raw["agents"] = serde_json::json!([]);
        }
        if case == "ambiguous" {
            let repeated = raw["agents"][0].clone();
            raw["agents"].as_array_mut().unwrap().push(repeated);
        }
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        if case == "unreadable" {
            std::fs::write(
                physical
                    .path()
                    .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                b"invalid agents JSON",
            )
            .unwrap();
        }
        let cfg = if case == "missing-physical" {
            test_settings()
        } else {
            crate::config::settings::load_settings_from_path(&path)
        };
        let expected_error = match case {
            "removed" => "coding_agent_binding_entry_missing_or_ambiguous",
            "ambiguous" => "coding_agent_identity_duplicate",
            "unreadable" => "coding_agent_identity_physical_unreadable",
            _ => "coding_agent_identity_physical_missing",
        };
        assert_eq!(
            crate::config::settings::bootstrap_binding_revision(&cfg, "claude").unwrap_err(),
            expected_error
        );
        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            Arc::new(ScriptedSpawnBackend::default()),
        )));
        let app = session_test_app(cfg, session_mgr.clone(), pty_mgr.clone());
        let transaction =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let original_recipe = crate::config::sessions_persistence::PersistedSession {
            name: case.into(),
            shell: "test-shell".into(),
            working_directory: cwd.path().to_string_lossy().to_string(),
            agent_id: Some("claude".into()),
            ..Default::default()
        };
        let sessions_path = physical.path().join("sessions.json");
        let bytes = serde_json::to_vec(&vec![original_recipe.clone()]).unwrap();
        std::fs::write(&sessions_path, &bytes).unwrap();
        let mut loaded =
            crate::config::sessions_persistence::load_sessions_from_dir(physical.path());
        assert_eq!(loaded.len(), 1);
        let mut recipe = loaded.remove(0);
        let loaded_id = recipe.coding_agent_binding_id.clone();
        let loaded_origin = recipe.coding_agent_binding_origin.clone();
        use sha2::{Digest, Sha256};
        assert_eq!(
            loaded_origin.as_ref().unwrap()["rawFileSHA256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(
            loaded_origin.as_ref().unwrap()["sessionsPath"],
            sessions_path.to_string_lossy().as_ref()
        );
        assert_eq!(loaded_origin.as_ref().unwrap()["originalOrdinal"], 0);
        let settings = app.state::<crate::config::settings::SettingsState>();
        let cfg = settings.read().await;
        assert!(recipe.capture_binding_seed(&cfg).is_err());
        drop(cfg);
        assert_eq!(recipe.coding_agent_binding_id, loaded_id);
        assert_eq!(recipe.coding_agent_binding_origin, loaded_origin);
        assert_eq!(recipe.shell, original_recipe.shell);
        let birth = recipe.binding_metadata().unwrap().unwrap();
        assert!(birth.seed.is_none());
        let info = super::create_session_inner_for_restore(
            &transaction,
            &session_mgr,
            &pty_mgr,
            recipe.shell.clone(),
            recipe.shell_args.clone(),
            recipe.working_directory.clone(),
            Some(recipe.name.clone()),
            recipe.agent_id.clone(),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            Some(false),
            None,
            Some(birth.clone()),
            None,
        )
        .await
        .unwrap();
        let manager = session_mgr.read().await.clone();
        assert_eq!(info.shell, recipe.shell);
        let restored = manager
            .coding_agent_binding(Uuid::parse_str(&info.id).unwrap())
            .await
            .unwrap();
        assert_eq!(restored.id, birth.id);
        assert_eq!(restored.seed, birth.seed);
        assert_eq!(restored.origin, birth.origin);
        let snapshot = crate::config::sessions_persistence::snapshot_sessions(&manager).await;
        let stored = snapshot
            .iter()
            .find(|row| row.id.as_deref() == Some(info.id.as_str()))
            .unwrap();
        assert_eq!(
            stored.coding_agent_binding_id,
            recipe.coding_agent_binding_id
        );
        assert_eq!(
            stored.coding_agent_binding_origin,
            recipe.coding_agent_binding_origin
        );
        assert!(stored.coding_agent_binding_seed.is_none());
        drop(transaction);
        // Central creation adds no capture-only veto; public launch gates stay intact.
        let new_cwd = cwd.path().join("new-birth");
        std::fs::create_dir(&new_cwd).unwrap();
        let new = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".into(),
            Vec::new(),
            new_cwd.to_string_lossy().to_string(),
            Some("seedless new birth".into()),
            Some("claude".into()),
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
        .unwrap();
        let new_binding = manager
            .coding_agent_binding(Uuid::parse_str(&new.id).unwrap())
            .await;
        if case == "removed" {
            assert!(new.agent_id.is_none());
            assert!(new_binding.is_none());
        } else {
            assert_eq!(new.agent_id.as_deref(), Some("claude"));
            let new_binding = new_binding.unwrap();
            assert!(new_binding.seed.is_none());
            assert_ne!(new_binding.id, birth.id);
        }
        if case == "removed" {
            let settings = app.state::<crate::config::settings::SettingsState>();
            let restarted = super::restart_session_inner_with_activation(
                app.handle(),
                &session_mgr,
                &pty_mgr,
                settings.inner(),
                Uuid::parse_str(&info.id).unwrap(),
                None,
                None,
                Some(false),
                true,
            )
            .await
            .unwrap();
            let after = manager
                .coding_agent_binding(Uuid::parse_str(&restarted.id).unwrap())
                .await
                .unwrap();
            assert_eq!(after.id, birth.id);
            assert_eq!(after.seed, birth.seed);
            assert_eq!(after.origin, birth.origin);
        }
        close_test_coordinator(&app).await;
    }
}

pub(crate) fn test_settings() -> AppSettings {
    AppSettings {
        agents: vec![
            AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: "claude".to_string(),
                id_hash: None,
                id_hash_debug: None,
                label: "Claude Code".to_string(),
                command: "claude".to_string(),
                color: "#d97706".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            },
            AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: "codex".to_string(),
                id_hash: None,
                id_hash_debug: None,
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#10b981".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            },
        ],
        ..AppSettings::default()
    }
}

/// #2124 T14 - the pure launch resolver: session -> effective catalog entry
/// correspondence, then the burst subfields with the profile defaults.
#[test]
fn resolve_launch_idle_tuning_matches_the_effective_catalog_entry() {
    fn catalog_rows(rows: serde_json::Value) -> Vec<CodingAgentDefinition> {
        serde_json::from_value(rows).expect("catalog rows")
    }
    fn claude_entry(idle_burst: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "key": "claude",
            "label": "Claude Code",
            "description": "d",
            "color": "#111",
            "command": "claude",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": idle_burst,
        })
    }

    // A configured command with arguments matches by executable basename.
    let mut settings = test_settings();
    settings.agents[0].command = "claude --dangerously-skip-permissions".to_string();
    let catalog = catalog_rows(serde_json::json!([claude_entry(
        serde_json::json!({ "maxBytes": 2048 })
    )]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &catalog,
    );
    assert_eq!(resolved.burst_max_bytes, 2048);
    assert_eq!(
        resolved.burst_window,
        Duration::from_secs(3),
        "an absent maxSecs takes the default"
    );
    assert_eq!(resolved.burst_prior_silence, Duration::from_secs(60));
    // The no-catalog tuning is the per-kind default (T10 pins it), and the
    // burst subfields must not move `idle_threshold` or `resize_grace`.
    let per_kind =
        crate::session::profile::idle_tuning_for_launch(Some(CodingAgentKind::Claude), None);
    assert_eq!(resolved.idle_threshold, per_kind.idle_threshold);
    assert_eq!(resolved.resize_grace, per_kind.resize_grace);

    // #2156 - an entry with `idleBurst: null` means no override, so the
    // per-kind filter stays ON.
    let off = catalog_rows(serde_json::json!([claude_entry(serde_json::Value::Null)]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &off,
    );
    assert_eq!(resolved, per_kind);

    // No agent id, no configured agent, or no matching command: the
    // override is dropped. #2156 - the per-kind tuning then decides, so a
    // recognised kind keeps the filter ON and an unrecognised one is OFF.
    for (agent_id, command, agent_kind) in [
        (None, "claude", Some(CodingAgentKind::Claude)),
        (Some("nope"), "claude", Some(CodingAgentKind::Claude)),
        (Some("claude"), "mystery", Some(CodingAgentKind::Claude)),
        (None, "claude", None),
    ] {
        let mut probe = test_settings();
        probe.agents[0].command = command.to_string();
        let resolved = resolve_launch_idle_tuning(&probe, agent_id, agent_kind, &catalog);
        let expected = match agent_kind {
            Some(_) => crate::session::profile::IdleTuning::AGENT_DEFAULT,
            None => crate::session::profile::IdleTuning::DEFAULT,
        };
        assert_eq!(
            resolved, expected,
            "agent_id={agent_id:?} kind={agent_kind:?}"
        );
        if agent_kind.is_none() {
            assert_eq!(resolved.burst_max_bytes, 0);
        }
    }

    // Two entries share the basename; the second states the configured
    // command identically and must win.
    let catalog = catalog_rows(serde_json::json!([
        claude_entry(serde_json::json!({ "maxBytes": 111 })),
        {
            "key": "claude-beta",
            "label": "Claude beta",
            "description": "d",
            "color": "#222",
            "command": "claude --dangerously-skip-permissions",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": { "maxBytes": 222 }
        }
    ]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &catalog,
    );
    assert_eq!(resolved.burst_max_bytes, 222, "the identical command wins");
}

/// #2124 T15 - from the seeded files on disk to the resolved tuning: an
/// `agents.50.personal.no-git.json` edit applies at the next spawn without touching
/// `settings.agents[]`.
#[test]
fn resolve_launch_idle_tuning_reads_the_effective_catalog_from_disk() {
    use crate::config::coding_agents_catalog::{
        ensure_seeded_for_project, load_catalog_for_settings,
    };

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    ensure_seeded_for_project(project);
    let mut settings = test_settings();
    settings.project_paths = vec![project.to_string_lossy().to_string()];
    let resolve = || {
        let catalog = load_catalog_for_settings(&settings).expect("seeded catalog");
        resolve_launch_idle_tuning(
            &settings,
            Some("claude"),
            Some(CodingAgentKind::Claude),
            &catalog,
        )
    };

    let resolved = resolve();
    assert_eq!(resolved.burst_max_bytes, 1024);
    assert_eq!(resolved.burst_window, Duration::from_secs(3));
    assert_eq!(resolved.burst_prior_silence, Duration::from_secs(60));

    let local = project
        .join(".ac")
        .join("coding-agents")
        .join("agents.50.personal.no-git.json");
    std::fs::write(
        &local,
        r#"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":0}}]}"#,
    )
    .unwrap();
    assert_eq!(
        resolve().burst_max_bytes,
        0,
        "a local edit applies to the next spawn"
    );

    std::fs::write(
        &local,
        r#"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":1.5}}]}"#,
    )
    .unwrap();
    assert_eq!(resolve().burst_window, Duration::from_millis(1500));
    assert_eq!(
        settings.agents[0].command, "claude",
        "settings.agents[] was never touched"
    );
}

#[test]
fn resolve_launch_auto_self_clear_uses_capability_and_setting_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("__agent_dev-rust");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.to_string_lossy().to_string();
    let mut settings = AppSettings::default();

    assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
    assert!(!resolve_launch_auto_self_clear(
        &settings, "pi", &cwd, false
    ));

    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, false));
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), false);
    assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));

    settings.auto_self_clear_by_agent.clear();
    for shell in ["claude", "codex-wrapper", "agy"] {
        assert!(resolve_launch_auto_self_clear(&settings, shell, &cwd, true));
    }
    for shell in ["agent", "cmd.exe", "pwsh", "pip", "pi-agent"] {
        assert!(!resolve_launch_auto_self_clear(
            &settings, shell, &cwd, true
        ));
    }

    settings.auto_self_clear_enabled = false;
    assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
    assert!(!resolve_launch_auto_self_clear(
        &settings, "claude", &cwd, true
    ));
}

#[test]
fn configured_pi_materialization_wires_auto_self_clear_to_agents_md() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    let cwd = matrix.to_string_lossy().to_string();
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "pi".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Pi".to_string(),
            command: "pi".to_string(),
            color: "#10b981".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: Some("AGENTS.md".to_string()),
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    let target = crate::config::agent_command::resolve_target_filename(Some("pi"), &settings, None);
    assert_eq!(target.as_deref(), Some("AGENTS.md"));
    assert_eq!(
        crate::config::agent_command::resolve_target_filename(None, &settings, None),
        None,
        "an ad hoc Pi launch has no managed target filename"
    );
    let managed = crate::config::agent_command::managed_instructions_filenames(&settings);
    let enabled = resolve_launch_auto_self_clear(&settings, "pi", &cwd, true);
    assert!(enabled);

    crate::config::session_context::materialize_agent_context_file_with_filename(
        &cwd,
        target.as_deref().unwrap(),
        &managed,
        true,
        enabled,
        None,
    )
    .unwrap()
    .expect("configured Pi context should materialize");
    let on = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
    assert_eq!(on.matches("## Self-Maintenance").count(), 1);

    crate::config::session_context::materialize_agent_context_file_with_filename(
        &cwd,
        "AGENTS.md",
        &managed,
        true,
        false,
        None,
    )
    .unwrap()
    .expect("configured Pi context should rematerialize");
    let off = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
    assert!(!off.contains("## Self-Maintenance"));
}

fn inert_pi_spawn() -> crate::config::agent_command::AgentSpawnCommand {
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "pi".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Pi".to_string(),
            command: "pi".to_string(),
            color: "#10b981".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    crate::config::agent_command::resolve_agent_spawn_command(&settings, "pi", None, None, false)
        .expect("Pi test spawn should resolve without filesystem preparation")
}

fn probe_map() -> ContainerPathMap {
    ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
}

fn norm(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn pi_provider_values_never_resolve_a_claude_probe() {
    for args in [
        vec![
            "--provider".to_string(),
            "claude".to_string(),
            "--continue".to_string(),
        ],
        vec!["--model".to_string(), "claude-sonnet".to_string()],
    ] {
        assert_eq!(
            CodingAgentKind::detect("pi", &args),
            Some(CodingAgentKind::Pi)
        );
        let probe = claude_resume_probe_target_for_kind(
            Some(CodingAgentKind::Pi),
            SessionBackendKind::LocalProcess,
            None,
            None,
            None,
            "pi",
            &args,
            r"Z:\path-that-must-not-be-probed",
        );
        assert!(probe.is_none());
    }
}

#[test]
fn resume_probe_target_local_uses_effective_config_dir_on_host() {
    let cwd = r"C:\Users\maria\repo\.ac\wg-1\__agent_x";
    let config_dir = r"C:\Users\maria\.claude-work";
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        Some(config_dir),
        "claude",
        &[],
        cwd,
    );

    assert_eq!(got.filesystem, "host");
    assert_eq!(
        got.host_probe_path,
        Some(claude_projects_dir_for_config_dir(config_dir, cwd))
    );
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_local_bare_claude_uses_default_resolver() {
    let cwd = r"C:\Users\Test\repo";
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        None,
        "claude",
        &[],
        cwd,
    );

    let expected = resolve_claude_projects_dir("claude", &[], cwd);
    let Some(expected) = expected else {
        return;
    };
    assert_eq!(got.filesystem, "host");
    assert_eq!(got.host_probe_path, Some(expected));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_local_wrapper_uses_wrapper_config_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let cwd = r"C:\Users\Test\repo";

    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        None,
        wrapper.to_str().unwrap(),
        &[],
        cwd,
    );

    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(got.filesystem, "host");
    assert_eq!(got.host_probe_path, Some(expected));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_container_no_value_warns_and_skips_probe() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        None,
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-unreachable");
    assert!(got.host_probe_path.is_none());
    assert_eq!(
        got.warning.as_ref().map(|w| w.kind),
        Some(WARNING_KIND_NO_VALUE)
    );
}

#[test]
fn resume_probe_target_container_maps_bind_mounted_config_dir() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        Some(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude"),
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-via-mount");
    let host_probe = got.host_probe_path.expect("host probe");
    assert!(norm(&host_probe).ends_with("/__agent_x/.claude/projects/-workspace"));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_container_unmappable_config_dir_skips_probe_without_duplicate_warning() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        Some("/workspace/.claude"),
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-unreachable");
    assert!(got.host_probe_path.is_none());
    assert!(got.warning.is_none());
}

fn cred_host_root() -> &'static str {
    if cfg!(windows) {
        r"C:\Users\maria\repo\.ac\wg-1\__agent_x"
    } else {
        "/Users/maria/repo/.ac/wg-1/__agent_x"
    }
}

fn cred_plan_map() -> ContainerPathMap {
    ContainerPathMap::new(cred_host_root(), "/workspace").unwrap()
}

fn cred_plan() -> ContainerCredentialPlan {
    let dest = std::path::Path::new(cred_host_root())
        .join(".claude")
        .join(".credentials.json");
    ContainerCredentialPlan {
        source: PathBuf::from("unused-host-source"),
        dest,
        first_run: None,
    }
}

#[test]
fn injected_config_dir_defaults_to_copy_dir_and_maps_into_container() {
    // #930 - host-login copy will happen (plan Some) and the user set no
    // CLAUDE_CONFIG_DIR -> inject the copy dir (host path), which the container
    // env translation maps to /workspace/.claude so the copied token is read.
    let plan = cred_plan();
    let injected = injected_claude_config_dir_for_copy(Some(&plan), false)
        .expect("copy without a user value must inject the copy dir");
    let expected_dir = format!("{}/.claude", cred_host_root().replace('\\', "/"));
    assert_eq!(injected.replace('\\', "/"), expected_dir);

    let translated = container_child_env(
        vec![(CLAUDE_CONFIG_DIR_KEY.to_string(), injected)],
        Vec::new(),
        &cred_plan_map(),
    );
    assert_eq!(
        translated.child_env,
        vec![(
            CLAUDE_CONFIG_DIR_KEY.to_string(),
            "/workspace/.claude".to_string()
        )]
    );
    assert!(translated.env_unset.is_empty());
    assert!(translated.warnings.is_empty());
}

#[test]
fn injected_config_dir_respects_explicit_user_value() {
    // #930 - the user already configured CLAUDE_CONFIG_DIR; never overwrite it,
    // even though a copy plan exists.
    assert_eq!(
        injected_claude_config_dir_for_copy(Some(&cred_plan()), true),
        None
    );
}

#[test]
fn injected_config_dir_none_without_copy_plan() {
    // #930 - host-login-reuse off or no host creds => no plan => inject nothing,
    // even when the user set no CLAUDE_CONFIG_DIR.
    assert_eq!(injected_claude_config_dir_for_copy(None, false), None);
}

#[test]
fn container_path_context_uses_canonical_host_root_for_guard_and_map() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp
        .path()
        .join("project")
        .join(".ac")
        .join("wg-1-team")
        .join("repo-foo");
    std::fs::create_dir_all(&repo).unwrap();
    let raw = repo.join(".");
    let canonical = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
        &std::fs::canonicalize(&repo).unwrap(),
    );

    let got = container_path_context_for_cwd(raw.to_str().unwrap()).unwrap();

    assert_eq!(got.host_root, canonical);
    assert_eq!(got.map.host_root(), canonical);
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .expect("failed to invoke mklink");
    assert!(
        output.status.success(),
        "failed to create junction link='{}' target='{}': stdout={} stderr={}",
        link.display(),
        target.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_junction_targeting_workgroup_root() {
    // B4 regression fence: before cwd canonicalization, the textual guard
    // saw only `link-to-wg` and permitted a bind mount of the workgroup.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp
        .path()
        .join("project")
        .join(".ac")
        .join("wg-11-dev-v4-team");
    std::fs::create_dir_all(&target).unwrap();
    let link = tmp.path().join("link-to-wg");
    create_junction(&link, &target);

    let err = container_path_context_for_cwd(link.to_str().unwrap())
        .expect_err("junction to workgroup root must be refused");

    // #1614 section 9.3 clause 1: the string this pins is produced in a
    // DIFFERENT file, pty/container_paths.rs, which section 5.3 renames
    // under Rule R. The fixture below stays a legacy `wg-*` directory
    // (Rule P2) precisely so this proves the refusal still fires for one.
    assert!(err.contains("room root"), "{err}");
    assert!(err.contains("selected path"), "{err}");
    assert!(err.contains("canonical path"), "{err}");
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_junction_targeting_home_dir() {
    // Guard coverage, not the main B4 regression fence: the pre-B4 home
    // rule already canonicalized both sides. This test mainly preserves the
    // selected-vs-canonical rejection text for reparse-point inputs.
    //
    // Safety note: `%TEMP%` usually lives under the user's home directory,
    // so this junction points from a tempdir back to its own ancestor.
    // The test relies on std::fs::remove_dir_all treating the junction as a
    // reparse point and deleting only the link, not descending into home.
    let tmp = tempfile::tempdir().unwrap();
    let home = dirs::home_dir().expect("home dir required for junction guard test");
    let link = tmp.path().join("link-to-home");
    create_junction(&link, &home);

    let err = container_path_context_for_cwd(link.to_str().unwrap())
        .expect_err("junction to home dir must be refused");

    assert!(err.contains("home directory"), "{err}");
    assert!(err.contains("selected path"), "{err}");
    assert!(err.contains("canonical path"), "{err}");
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_cwd_that_cannot_be_canonicalized() {
    // Pins the fail-closed DD9 behavior from 8d0d3fd5. It is not the B4
    // junction bypass regression fence.
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");

    let err = container_path_context_for_cwd(missing.to_str().unwrap())
        .expect_err("missing cwd must be refused");

    assert!(
        err.contains("failed to canonicalize container mount source"),
        "{err}"
    );
    assert!(err.contains(missing.to_str().unwrap()), "{err}");
}

#[derive(Default)]
struct FailingSpawnBackend {
    spawned: Mutex<Vec<Uuid>>,
    killed: Mutex<Vec<Uuid>>,
    /// #973 - every (cols, rows) a spawn was asked for.
    sizes: Mutex<Vec<(u16, u16)>>,
}

impl FailingSpawnBackend {
    fn spawned(&self) -> Vec<Uuid> {
        self.spawned.lock().unwrap().clone()
    }

    fn killed(&self) -> Vec<Uuid> {
        self.killed.lock().unwrap().clone()
    }

    /// #973 - the sizes the PTY would have been opened at.
    fn sizes(&self) -> Vec<(u16, u16)> {
        self.sizes.lock().unwrap().clone()
    }
}

impl crate::pty::backend::PtyBackend for FailingSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.spawned.lock().unwrap().push(spec.id);
            // #973 - the size the ConPTY would have been opened at.
            self.sizes.lock().unwrap().push((spec.cols, spec.rows));
            Err(crate::errors::AppError::PtyError(
                "synthetic spawn failure".to_string(),
            ))
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.killed.lock().unwrap().push(id);
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
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

/// A `PtyBackend` whose `spawn` parks on a oneshot so a test can observe the
/// `create_session_inner` spawn mark while the PTY is still being spawned.
/// `fail` decides whether the parked spawn ultimately errors (exercising the
/// rollback path) or succeeds and records the session as live.
struct GatedSpawnBackend {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    live: Mutex<Vec<Uuid>>,
    fail: bool,
}

impl GatedSpawnBackend {
    fn new(
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
        fail: bool,
    ) -> Self {
        Self {
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(release)),
            live: Mutex::new(Vec::new()),
            fail,
        }
    }
}

impl crate::pty::backend::PtyBackend for GatedSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        let started = self.started.lock().unwrap().take().expect("started sender");
        let release = self
            .release
            .lock()
            .unwrap()
            .take()
            .expect("release receiver");
        let fail = self.fail;
        Box::pin(async move {
            // Announce that PtyManager::spawn has released the outer manager
            // mutex and is now parked inside the backend, then wait for the
            // test to inspect the spawn mark before the spawn resolves.
            let _ = started.send(());
            let _ = release.await;
            if fail {
                Err(crate::errors::AppError::PtyError(
                    "synthetic spawn failure".to_string(),
                ))
            } else {
                self.live.lock().unwrap().push(spec.id);
                Ok(())
            }
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().retain(|live| *live != id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
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

#[derive(Default)]
pub(crate) struct ScriptedSpawnBackend {
    live: Mutex<HashSet<Uuid>>,
    spawned: Mutex<Vec<Uuid>>,
    spawn_count: AtomicUsize,
    fail_spawn_number: AtomicUsize,
    gate_spawn_number: AtomicUsize,
    gate_started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
    gate_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct BarrierDestroyBackend {
    live: Mutex<HashSet<Uuid>>,
    fail_kill: Mutex<HashSet<Uuid>>,
    kills: Mutex<Vec<Uuid>>,
    started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl BarrierDestroyBackend {
    fn new(
        started: tokio::sync::oneshot::Sender<Uuid>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Self {
        Self {
            live: Mutex::new(HashSet::new()),
            fail_kill: Mutex::new(HashSet::new()),
            kills: Mutex::new(Vec::new()),
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(release)),
        }
    }

    fn fail_kill_for(&self, session_id: Uuid) {
        self.fail_kill.lock().unwrap().insert(session_id);
    }
}

impl crate::pty::backend::PtyBackend for BarrierDestroyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
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
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(id);
            if let Some(release) = self.release.lock().unwrap().take() {
                release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|error| {
                        crate::errors::AppError::PtyError(format!(
                            "destroy barrier release failed: {error}"
                        ))
                    })?;
            }
        }
        if self.fail_kill.lock().unwrap().contains(&id) {
            return Err(crate::errors::AppError::PtyError(
                "synthetic live teardown failure".to_string(),
            ));
        }
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

impl ScriptedSpawnBackend {
    pub(crate) fn fail_spawn(&self, number: usize) {
        self.fail_spawn_number.store(number, Ordering::SeqCst);
    }

    fn gate_spawn(
        &self,
        number: usize,
        started: tokio::sync::oneshot::Sender<Uuid>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        self.gate_spawn_number.store(number, Ordering::SeqCst);
        *self.gate_started.lock().unwrap() = Some(started);
        *self.gate_release.lock().unwrap() = Some(release);
    }

    fn lose_route(&self, session_id: Uuid) {
        self.live.lock().unwrap().remove(&session_id);
    }
}

impl crate::pty::backend::PtyBackend for ScriptedSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            let number = self.spawn_count.fetch_add(1, Ordering::SeqCst) + 1;
            self.spawned.lock().unwrap().push(spec.id);
            if self.gate_spawn_number.load(Ordering::SeqCst) == number {
                if let Some(started) = self.gate_started.lock().unwrap().take() {
                    let _ = started.send(spec.id);
                }
                let release = self.gate_release.lock().unwrap().take();
                if let Some(release) = release {
                    let _ = release.await;
                }
            }
            if self.fail_spawn_number.load(Ordering::SeqCst) == number {
                return Err(crate::errors::AppError::PtyError(
                    "synthetic scripted spawn failure".to_string(),
                ));
            }
            self.live.lock().unwrap().insert(spec.id);
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

#[derive(Clone, Copy)]
pub(crate) enum SessionTestStoreState {
    Ready,
    Error,
    Missing,
}

pub(crate) fn session_test_app_with_store(
    settings: AppSettings,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
    store_state: SessionTestStoreState,
) -> tauri::App<tauri::test::MockRuntime> {
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown.token().clone(),
    );
    let output_senders = Arc::new(Mutex::new(HashMap::new()));
    // #2232 phase 4 test 9: the app owns the **single** capture registry and
    // an outbound network, exactly as `run()` does, so a supervisor-spawned
    // reader reaches the registry slot and a bot attach can be counted.
    let captures = Arc::new(crate::capture::registry::CaptureRegistry::new());
    let telegram: crate::telegram::manager::TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::with_captures(
                output_senders,
                Arc::clone(&captures),
            ),
        ));
    let store_dir = tempfile::TempDir::new().expect("create target-gate store");
    let store_root = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(store_dir.path()).expect("canonicalize target-gate store"),
    );
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_root.join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open target-gate store"),
    );
    let target_gate_state =
        crate::api::message_store::PtyInputTargetGateState::for_root(store_root);
    let mut builder = tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::new(
            crate::resource_monitor::ResourceMonitorState::new(),
        ))
        .manage(session_mgr)
        .manage(pty_mgr)
        .manage(crate::DetachedSessionsState::default())
        .manage(telegram)
        .manage(Arc::clone(&captures))
        .manage(crate::network::OutboundNetwork::new_for_tests(1))
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
        .manage(coordinator.clone())
        .manage(target_gate_state.clone())
        .manage(shutdown);
    builder = match store_state {
        SessionTestStoreState::Ready => builder.manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Ok(message_store),
                target_gate_state.gate.clone(),
            ),
        ),
        SessionTestStoreState::Error => builder.manage(
            crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                Err("store_unavailable".to_string()),
                target_gate_state.gate.clone(),
            ),
        ),
        SessionTestStoreState::Missing => builder,
    };
    let app = builder
        .manage(store_dir)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build session test app");
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

fn session_test_app(
    settings: AppSettings,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
) -> tauri::App<tauri::test::MockRuntime> {
    session_test_app_with_store(settings, session_mgr, pty_mgr, SessionTestStoreState::Ready)
}

fn strict_target_fixture() -> (tempfile::TempDir, String, String) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let team = ac_root.join("_team_team");
    let coordinator_matrix = ac_root.join("_agent_lead");
    let first_matrix = ac_root.join("_agent_dev-one");
    let second_matrix = ac_root.join("_agent_dev-two");
    let workgroup = ac_root.join("wg-1-team");
    let coordinator = workgroup.join("__agent_lead");
    let first = workgroup.join("__agent_dev-one");
    let second = workgroup.join("__agent_dev-two");
    for directory in [
        &team,
        &coordinator_matrix,
        &first_matrix,
        &second_matrix,
        &coordinator,
        &first,
        &second,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
            team.join("config.json"),
            r#"{"agents":["../_agent_dev-one","../_agent_dev-two","../_agent_lead"],"coordinator":"../_agent_lead"}"#,
        )
        .unwrap();
    for (replica, identity) in [
        (&coordinator, "../../_agent_lead"),
        (&first, "../../_agent_dev-one"),
        (&second, "../../_agent_dev-two"),
    ] {
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"{identity}"}}"#),
        )
        .unwrap();
    }
    (
        temp,
        first.to_string_lossy().into_owned(),
        second.to_string_lossy().into_owned(),
    )
}

/// #1175. The bytes every rotation fixture seeds into `memory/MEMORY.md`.
/// Asserted verbatim on both sides of every differential, so a green test can
/// never mean "an archive directory exists but is empty".
const ROTATION_SENTINEL: &str = "remembered bytes";

/// #1175. One replica and the origin Agent Matrix its `config.json` identity
/// points at.
struct RotationSide {
    replica_cwd: String,
    matrix: std::path::PathBuf,
}

/// #1175. `strict_target_fixture` (`:5896`) already builds two symmetric
/// replicas under `<temp>/project/.ac/wg-1-team/` whose `config.json` identity
/// names `<temp>/project/.ac/_agent_dev-one` and `_agent_dev-two`. Those two
/// directories sit directly under a `.ac` workspace, so they satisfy
/// `is_canonical_agent_matrix_dir`, which is what `resolve_rotatable_matrix_root`
/// (`config/agent_memory.rs:83`) resolves a replica session to.
///
/// Seed a non-empty `memory/` in each: #1172 D2 makes an EMPTY `memory/` a
/// no-op, so the sentinel is what gives the fresh half of each differential
/// something to rotate.
fn rotation_fixture() -> (tempfile::TempDir, RotationSide, RotationSide) {
    let (temp, first_replica, second_replica) = strict_target_fixture();
    let ac_root = temp.path().join("project").join(".ac");
    let mut sides = Vec::new();
    for (replica_cwd, agent) in [
        (first_replica, "_agent_dev-one"),
        (second_replica, "_agent_dev-two"),
    ] {
        let matrix = ac_root.join(agent);
        std::fs::create_dir_all(matrix.join("memory")).expect("create origin memory/");
        std::fs::write(matrix.join("memory").join("MEMORY.md"), ROTATION_SENTINEL)
            .expect("seed origin memory/");
        sides.push(RotationSide {
            replica_cwd,
            matrix,
        });
    }
    let second = sides.pop().expect("second side");
    let first = sides.pop().expect("first side");
    (temp, first, second)
}

/// #1175. Every nested memory rotation, sorted. Same shape as
/// `agent_memory.rs:180-189`.
fn rotated_memory_dirs(matrix: &std::path::Path) -> Vec<String> {
    let entries = match std::fs::read_dir(matrix.join("memory-archive")) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => panic!("read origin memory archive: {err}"),
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.expect("dir entry").file_name())
        .filter_map(|name| name.to_str().map(str::to_string))
        .filter(|name| name.starts_with("memory_"))
        .collect();
    names.sort();
    names
}

/// #1175. "Nothing has happened to this side." Used for the resume side AND
/// for a side whose own launch has not run yet, which is what stops the second
/// launch from laundering a side effect of the first (D3, hole 2).
fn assert_memory_pristine(side: &RotationSide, case: &str) {
    assert!(
        std::fs::read_dir(&side.matrix)
            .expect("read matrix")
            .all(|entry| !entry
                .expect("matrix entry")
                .file_name()
                .to_string_lossy()
                .starts_with("memory_")),
        "{case}: no root archive may be created"
    );
    assert_eq!(
        std::fs::read_to_string(side.matrix.join("memory").join("MEMORY.md")).ok(),
        Some(ROTATION_SENTINEL.to_string()),
        "#1175 ({case}): the origin matrix's live memory/ must be exactly as it was"
    );
    let rotated = rotated_memory_dirs(&side.matrix);
    assert!(
        rotated.is_empty(),
        "#1175 ({case}): nothing may have rotated, and this created {rotated:?} in {}",
        side.matrix.display()
    );
    let replica = std::path::Path::new(&side.replica_cwd);
    assert!(
        rotated_memory_dirs(replica).is_empty()
            && !replica.join("memory").exists()
            && !replica.join("memory-archive").exists(),
        "#1175 ({case}): the replica itself must gain no memory* entry"
    );
}

/// #1175. The RESUME assertion: pristine, PLUS a witness that the launch
/// actually entered the context block.
///
/// The witness is not decoration. `:2041` is an `if let`, and a launch that
/// skips it still reaches the spawn, so `spawn_count` cannot see the
/// difference; without this, "nothing rotated" is satisfiable by a resume that
/// never reached the gate at all.
/// `materialize_agent_context_file_with_filename_activated`
/// (`config/session_context.rs:2215`) writes `cwd.join(target_filename)`, and
/// the fixture seeds only `config.json` into a replica, so this file existing
/// means the block ran. Measured in probe 14 and in 2.7.
fn assert_resume_left_memory_alone(side: &RotationSide, case: &str) {
    assert!(
        std::path::Path::new(&side.replica_cwd)
            .join("AGENTS.md")
            .is_file(),
        "#1175 ({case}): the resume must have ENTERED the context block; without \
             this witness the no-rotation assertion below can pass vacuously"
    );
    assert_memory_pristine(side, case);
}

/// #1175. The FRESH assertion, which is also the POSITIVE CONTROL: it fails if
/// the launch never reached the rotation chokepoint, which is what makes a green
/// `assert_resume_left_memory_alone` on the resume side attributable to the gate
/// rather than to an unreached call site.
fn assert_memory_rotated_once(side: &RotationSide, case: &str) {
    assert!(
        std::fs::read_dir(&side.matrix)
            .expect("read matrix")
            .all(|entry| !entry
                .expect("matrix entry")
                .file_name()
                .to_string_lossy()
                .starts_with("memory_")),
        "{case}: no root archive may be created"
    );
    let rotated = rotated_memory_dirs(&side.matrix);
    assert_eq!(
        rotated.len(),
        1,
        "#1175 ({case}): a fresh launch must rotate exactly once, found {rotated:?}"
    );
    assert_eq!(
        std::fs::read_to_string(
            side.matrix
                .join("memory-archive")
                .join(&rotated[0])
                .join("MEMORY.md")
        )
        .ok(),
        Some(ROTATION_SENTINEL.to_string()),
        "#1175 ({case}): the archive must carry the previous session's bytes"
    );
    let live = side.matrix.join("memory");
    assert!(
        live.is_dir(),
        "#1175 ({case}): the fresh session must get a live memory/ back"
    );
    assert_eq!(
        std::fs::read_dir(&live).expect("read live memory/").count(),
        0,
        "#1175 ({case}): the recreated memory/ starts empty"
    );
    let replica = std::path::Path::new(&side.replica_cwd);
    assert!(
        rotated_memory_dirs(replica).is_empty()
            && !replica.join("memory").exists()
            && !replica.join("memory-archive").exists(),
        "#1175 ({case}): the replica itself must gain no memory* entry"
    );
}

async fn create_target_for_test(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    cwd: &str,
    intent: CreateSelectionIntent,
) -> Result<SessionInfo, String> {
    super::create_session_inner(
        app.handle(),
        session_mgr,
        pty_mgr,
        "codex".to_string(),
        Vec::new(),
        cwd.to_string(),
        Some("target gate fixture".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        intent,
    )
    .await
}

#[tokio::test]
async fn e2992_p42_invalid_prevents_spawn() {
    use crate::config::settings::{SettingsAvailability, SettingsState};

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(test_settings(), session_mgr.clone(), pty_mgr.clone());
    let cwd = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(cwd.path()).expect("canonicalize P42 session cwd"),
    )
    .to_string_lossy()
    .to_string();
    let ready = create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd).await;
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

    // Closed exercises the same Unavailable admission error as Invalid while
    // the earlier archive/creation gates still use StagedCompatibility reads.
    // Those gates are outside P42; no active-owner forwarding is used here.
    let settings = app.state::<SettingsState>();
    settings.drain_blocking(std::time::Duration::ZERO);
    assert_eq!(settings.availability(), SettingsAvailability::Closed);
    let error = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "test-shell".to_string(),
        Vec::new(),
        cwd,
        Some("unavailable".to_string()),
        Some("claude".to_string()),
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
    .unwrap_err();
    assert_eq!(error, "settings owner unavailable (Closed)");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, ready.id);
    close_test_coordinator(&app).await;
}

pub(crate) async fn create_scripted_session(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    cwd: &str,
) -> SessionInfo {
    super::create_session_inner(
        app.handle(),
        session_mgr,
        pty_mgr,
        "test-shell".to_string(),
        Vec::new(),
        cwd.to_string(),
        Some("restart fixture".to_string()),
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
    .expect("create scripted session")
}

fn capture_session_lifecycle(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<(String, String)> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    for event_name in [
        "session_created",
        "session_destroyed",
        "session_switched",
        "session_communication_changed",
    ] {
        let sender = sender.clone();
        app.listen_any(event_name, move |event| {
            let _ = sender.send((event_name.to_string(), event.payload().to_string()));
        });
    }
    receiver
}

pub(crate) async fn close_test_coordinator(app: &tauri::App<tauri::test::MockRuntime>) {
    use tauri::Manager;

    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

// #1063: while a real create is paused at `before_project_gate` with its pending
// row inserted, the deletion-only pending-inclusive snapshot must observe its
// working directory even though every public read hides it. This is the exact
// property the Agent Matrix delete recheck relies on.
#[tokio::test]
async fn before_project_gate_barrier_exposes_pending_create_to_deletion_snapshot_only() {
    let cwd = tempfile::tempdir().unwrap();
    let cwd_str = cwd.path().to_string_lossy().into_owned();
    // The create normalizes its cwd; key the barriers by the same value.
    let barrier_key = crate::path_utils::normalize_windows_verbatim_path(&cwd_str);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    let before_gate = super::seed_race_barriers::install_before_project_gate(&barrier_key);
    let after_seed = super::seed_race_barriers::install_after_seed_before_pty(&barrier_key);

    let create = {
        let app = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        let cwd_str = cwd_str.clone();
        tokio::spawn(async move {
            super::create_session_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                "codex".to_string(),
                Vec::new(),
                cwd_str,
                Some("race fixture".to_string()),
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
        })
    };

    let assert_pending_visible_to_deletion_only = || {
        let session_mgr = Arc::clone(&session_mgr);
        let cwd_str = cwd_str.clone();
        async move {
            assert!(
                session_mgr.read().await.list_sessions().await.is_empty(),
                "the pending create must stay hidden from public reads"
            );
            let workdirs = tokio::task::spawn_blocking(move || {
                session_mgr
                    .blocking_read()
                    .live_working_directories_for_deletion_blocking()
            })
            .await
            .unwrap();
            assert!(
                    workdirs.iter().any(|dir| dir == &cwd_str),
                    "the deletion-only snapshot must include the pending create workdir; got {workdirs:?}"
                );
        }
    };

    // Paused at before_project_gate, pending row inserted.
    before_gate.reached.notified().await;
    assert_pending_visible_to_deletion_only().await;
    before_gate.release.notify_one();

    // Paused at after_seed_before_pty, still pending and unfinalized.
    after_seed.reached.notified().await;
    assert_pending_visible_to_deletion_only().await;
    after_seed.release.notify_one();

    // Release the create and drive its PTY spawn to completion.
    started_rx.await.unwrap();
    release_tx.send(()).unwrap();
    create
        .await
        .expect("join create")
        .expect("create completes");
    close_test_coordinator(&app).await;
}

async fn create_manual_cascade_fixture(
    app: &tauri::App<tauri::test::MockRuntime>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    root: &std::path::Path,
) -> [Uuid; 4] {
    let mut ids = Vec::new();
    for name in ["external", "member-a", "member-b", "coordinator"] {
        let cwd = root.join(name);
        std::fs::create_dir_all(&cwd).unwrap();
        let info = create_scripted_session(app, session_mgr, pty_mgr, &cwd.to_string_lossy()).await;
        ids.push(Uuid::parse_str(&info.id).unwrap());
    }
    ids.try_into().expect("four cascade fixture ids")
}

#[tokio::test]
async fn manual_coordinator_cascade_barrier_publishes_only_one_final_selection() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let [external, member_a, member_b, coordinator_id] =
        create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
    app.state::<crate::session::selection::SelectionCoordinator>()
        .transition(crate::session::selection::SelectionRequest::user_switch(
            member_a,
        ))
        .await
        .unwrap();
    let events = capture_session_lifecycle(&app);
    let close = {
        let app = app.handle().clone();
        tokio::spawn(async move {
            execute_manual_coordinator_destroy(&app, coordinator_id, vec![member_a, member_b], true)
                .await
        })
    };

    assert_eq!(started_rx.await.unwrap(), member_a);
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(member_a),
        "selection must remain stable while the cascade is barrier-held"
    );
    assert!(events.try_recv().is_err());
    release_tx.send(()).unwrap();
    let outcome = close
        .await
        .expect("join cascade close")
        .expect("cascade succeeds");
    assert_eq!(
        outcome.destroyed_ids,
        vec![member_a, member_b, coordinator_id]
    );
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(external));
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::ManualClose
    );
    let observed = (0..4)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            "session_destroyed",
            "session_destroyed",
            "session_destroyed",
            "session_switched",
        ]
    );
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn manual_coordinator_cascade_failed_selected_member_stays_selected_without_fallback() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let [_external, member_a, member_b, coordinator_id] =
        create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
    app.state::<crate::session::selection::SelectionCoordinator>()
        .transition(crate::session::selection::SelectionRequest::user_switch(
            member_a,
        ))
        .await
        .unwrap();
    backend.fail_kill_for(member_a);
    let revision = session_mgr
        .read()
        .await
        .selection_payload()
        .await
        .revision();
    let events = capture_session_lifecycle(&app);
    let close = {
        let app = app.handle().clone();
        tokio::spawn(async move {
            execute_manual_coordinator_destroy(&app, coordinator_id, vec![member_a, member_b], true)
                .await
        })
    };
    assert_eq!(started_rx.await.unwrap(), member_a);
    release_tx.send(()).unwrap();
    let outcome = close
        .await
        .expect("join failed-member cascade")
        .expect("coordinator still closes");

    assert!(outcome
        .failed
        .iter()
        .any(|(session_id, error)| *session_id == member_a
            && error.contains("synthetic live teardown failure")));
    assert_eq!(outcome.destroyed_ids, vec![member_b, coordinator_id]);
    assert!(session_mgr
        .read()
        .await
        .get_session(member_a)
        .await
        .is_some());
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(member_a));
    assert_eq!(selection.revision(), revision);
    let observed = (0..2)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(observed, vec!["session_destroyed", "session_destroyed"]);
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_success_publishes_created_destroyed_selection_in_order() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    let carried = SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-07-16T00:00:00Z".to_string(),
        message: None,
    };
    session_mgr
        .read()
        .await
        .set_communication_for_test(old_id, carried.clone())
        .await;
    let events = capture_session_lifecycle(&app);
    let settings = app.state::<crate::config::settings::SettingsState>();

    let replacement = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(false),
        true,
    )
    .await
    .expect("restart succeeds");
    let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(replacement_id, old_id);
    assert!(!backend.has_session(old_id));
    assert!(backend.has_session(replacement_id));
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, replacement.id);
    assert_eq!(replacement.communication, Some(carried));
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(replacement_id));
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::Restart
    );

    let observed = (0..3)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["session_created", "session_destroyed", "session_switched"]
    );
    assert!(events.try_recv().is_err());
    let created_payload: serde_json::Value = serde_json::from_str(&observed[0].1).unwrap();
    assert_eq!(created_payload["communication"]["kind"], "raiseHand");
    let selection_payload: serde_json::Value = serde_json::from_str(&observed[2].1).unwrap();
    assert_eq!(selection_payload["id"], replacement.id);
    assert_eq!(selection_payload["source"], "restart");
    assert_eq!(selection_payload["userInitiated"], true);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_of_live_wg_replica_session_does_not_sessionrace() {
    // #1101: `teardown_old_for_restart` retains the old canonical row (non-Exited)
    // until the atomic commit's `mutations.remove(uuid)`, which runs AFTER the
    // create-gate. For a live WG replica restart (Suppress intent + replica cwd) the
    // gate saw `appeared_session == true` and returned Err("sessionRace"). The gate
    // must be skipped for the restart replacement create. The existing restart tests
    // use a plain tempdir (not a WG replica), so they never entered this gate.
    let (_fixture, first_cwd, _second) = strict_target_fixture();
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

    // Seed a LIVE session at the strict WG replica cwd. User intent bypasses the
    // create-gate on create and establishes a non-Exited canonical row.
    let old = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live WG replica session");
    let old_id = Uuid::parse_str(&old.id).unwrap();

    let settings = app.state::<crate::config::settings::SettingsState>();
    let replacement = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,        // agent_id
        None,        // requested_profile
        Some(false), // skip_auto_resume
        true,        // activate_after
    )
    .await
    .unwrap_or_else(|e| panic!("restart of a live WG replica session must not fail; got Err({e})"));

    let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(replacement_id, old_id, "restart must replace the old row");
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1, "old row replaced, not duplicated");
    assert_eq!(rows[0].id, replacement.id);
    close_test_coordinator(&app).await;
}

// ──────────────────────────────────────────────────────────────────────
// #1535 nested-project sessionRace confinement (S1-S2)
// ──────────────────────────────────────────────────────────────────────

/// #1535 S1/S2 fixture: `strict_target_fixture` PLUS a nested project with
/// its own full workgroup inside `__agent_dev-one`:
/// `nested-proj/.ac/{_team_team, _agent_lead2, _agent_inner2,
/// wg-2-team/{__agent_lead2, __agent_inner2}}` and the #1535 bug-shaped
/// origin dir `nested-proj/.ac/_agent_phase0`. The nested replica
/// `config.json` identity is `../../_agent_inner2`; the nested team config
/// names the nested origin matrices.
fn nested_strict_target_fixture() -> (tempfile::TempDir, String, String, String) {
    let (temp, first_cwd, _second) = strict_target_fixture();
    let outer_replica = std::path::PathBuf::from(&first_cwd);

    let nested_ac_root = outer_replica.join("nested-proj").join(".ac");
    let nested_team = nested_ac_root.join("_team_team");
    let nested_wg = nested_ac_root.join("wg-2-team");
    let nested_inner2 = nested_wg.join("__agent_inner2");
    let nested_origin = nested_ac_root.join("_agent_phase0");
    for directory in [
        &nested_team,
        &nested_ac_root.join("_agent_lead2"),
        &nested_ac_root.join("_agent_inner2"),
        &nested_wg.join("__agent_lead2"),
        &nested_inner2,
        &nested_origin,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        nested_team.join("config.json"),
        r#"{"agents":["../_agent_lead2","../_agent_inner2"],"coordinator":"../_agent_lead2"}"#,
    )
    .unwrap();
    std::fs::write(
        nested_inner2.join("config.json"),
        r#"{"identity":"../../_agent_inner2"}"#,
    )
    .unwrap();

    (
        temp,
        first_cwd,
        nested_inner2.to_string_lossy().into_owned(),
        nested_origin.to_string_lossy().into_owned(),
    )
}

/// #1535 S1: a create in a nested project (origin dir under an ancestor
/// workgroup replica, the exact bug shape) must NOT sessionRace against
/// the ANCESTOR replica's live session (pre-fix: `Err("sessionRace")`).
#[tokio::test]
async fn nested_project_create_does_not_sessionrace_against_ancestor_live_session() {
    let (temp, first_cwd, _nested_replica, nested_origin) = nested_strict_target_fixture();
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

    // Seed a LIVE session at the OUTER replica; the ancestor's live session
    // must not gate a nested-project create.
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live ancestor WG replica session");

    let created = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_origin,
        CreateSelectionIntent::Background,
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "nested-project create must not sessionRace against the ancestor session; got Err({e})"
        )
    });
    assert_eq!(created.working_directory, nested_origin);
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 2, "ancestor + nested sessions both exist");
    assert!(
        rows.iter()
            .any(|row| row.working_directory == nested_origin),
        "the nested-project session row must exist"
    );
    let _ = temp;
    close_test_coordinator(&app).await;
}

/// #1535 S2: a nested project with its OWN replica (a) does not race
/// against the ancestor's live session, and (b) still gates against its
/// own replica's duplicate.
#[tokio::test]
async fn nested_project_create_gates_against_its_own_replica_only() {
    let (temp, first_cwd, nested_replica, nested_origin) = nested_strict_target_fixture();
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

    // (a) Live ancestor session + nested replica create -> Ok (the nested
    // replica's gate scopes to the nested project, never the ancestor).
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live ancestor WG replica session");
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::Background,
    )
    .await
    .unwrap_or_else(|e| {
        panic!("nested replica create must not race the ancestor session; got Err({e})")
    });

    // (b) A live session on the nested replica itself + another Background
    // create at the same nested replica -> the nested project's OWN
    // duplicate still sessionRaces.
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::User,
    )
    .await
    .expect("seed live nested replica session");
    let err = create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &nested_replica,
        CreateSelectionIntent::Background,
    )
    .await
    .expect_err("the nested replica's own duplicate must still gate");
    assert_eq!(err, "sessionRace");
    let _ = (temp, nested_origin);
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_after_route_loss_cleans_old_external_state_once_before_replacement() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    session_mgr
        .read()
        .await
        .set_telegram_bot_id(old_id, Some("old-bot".to_string()))
        .await;
    let telegram = app.state::<crate::telegram::manager::TelegramBridgeState>();
    telegram.lock().await.insert_test_bridge(old_id, "old-bot");
    app.state::<crate::DetachedSessionsState>()
        .lock()
        .unwrap()
        .insert(old_id);

    backend.lose_route(old_id);
    pty_mgr.lock().unwrap().remove_route_if_kind(
        old_id,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    );
    let (spawn_started_tx, spawn_started_rx) = tokio::sync::oneshot::channel();
    let (spawn_release_tx, spawn_release_rx) = tokio::sync::oneshot::channel();
    backend.gate_spawn(2, spawn_started_tx, spawn_release_rx);
    let events = capture_session_lifecycle(&app);
    let restart = {
        let app_handle = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        let settings = app
            .state::<crate::config::settings::SettingsState>()
            .inner()
            .clone();
        tokio::spawn(async move {
            super::restart_session_inner_with_activation(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                &settings,
                old_id,
                None,
                None,
                Some(false),
                true,
            )
            .await
        })
    };

    let replacement_id = spawn_started_rx.await.expect("replacement spawn starts");
    assert!(!telegram.lock().await.has_bridge(old_id));
    assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
    assert!(!app
        .state::<crate::DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&old_id));
    let mut route_loss = {
        let sender = app
            .state::<crate::session::selection::SelectionCoordinator>()
            .container_lifecycle_sender();
        tokio::spawn(async move { sender.route_lost(old_id, 91).await })
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut route_loss)
            .await
            .is_err(),
        "late route-loss reconciliation must serialize behind restart"
    );
    spawn_release_tx.send(()).unwrap();
    let replacement = restart
        .await
        .expect("join restart")
        .expect("restart succeeds after old route loss");
    assert_eq!(replacement.id, replacement_id.to_string());
    assert_eq!(
        route_loss
            .await
            .expect("join route-loss callback")
            .expect("late route-loss callback"),
        crate::session::selection::CriticalAdmissionOutcome::Completed(())
    );
    assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
    assert!(session_mgr.read().await.get_session(old_id).await.is_none());
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(replacement_id)
    );
    let observed = (0..3)
        .map(|_| {
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec!["session_created", "session_destroyed", "session_switched"]
    );
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_pre_teardown_failure_preserves_old_row_route_selection_and_events() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    let events = capture_session_lifecycle(&app);
    let settings = app.state::<crate::config::settings::SettingsState>();
    settings.write().await.archived_project_paths = vec![cwd.path().to_string_lossy().to_string()];

    let error = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("restart teardown must fail");
    assert!(
        error.contains("Cannot start a session in archived project"),
        "{error}"
    );
    assert!(backend.has_session(old_id));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
    assert_eq!(
        session_mgr.read().await.selection_payload().await.id(),
        Some(old_id)
    );
    assert!(events.try_recv().is_err());
    let tracker = pty_mgr.lock().unwrap().response_close_state();
    let authority = match super::prepare_lifecycle_entry_authority(
        app.handle(),
        Uuid::new_v4(),
        Some(old_id),
        false,
        None,
    )
    .await
    .expect("real lifecycle authority")
    {
        super::LifecycleAuthorityAdmission::Ready(authority) => authority,
        _ => panic!("healthy Foreign fixture must acquire real authority"),
    };
    assert!(authority.has_owned_claim());
    assert!(authority
        .tracker
        .as_ref()
        .is_some_and(|actual| actual.same_state(&tracker)));
    tracker.fail_closed();
    assert!(
        !authority.release(),
        "claimed failed tracker cannot certify release"
    );
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn restart_post_teardown_spawn_failure_destroys_old_and_publishes_one_null() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    let events = capture_session_lifecycle(&app);
    backend.fail_spawn(2);
    let settings = app.state::<crate::config::settings::SettingsState>();

    let error = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("replacement spawn must fail");
    assert!(
        error.contains("synthetic scripted spawn failure"),
        "{error}"
    );
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), None);
    assert_eq!(
        selection.source(),
        crate::session::selection::SelectionSource::Restart
    );
    assert!(!backend.has_session(old_id));

    let destroyed = events
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("old destroy event");
    let switched = events
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("restart null event");
    assert_eq!(destroyed.0, "session_destroyed");
    assert_eq!(switched.0, "session_switched");
    assert!(events.try_recv().is_err());
    let selection_payload: serde_json::Value = serde_json::from_str(&switched.1).unwrap();
    assert!(selection_payload["id"].is_null());
    assert_eq!(selection_payload["source"], "restart");
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn dormant_restart_failure_retains_exact_exit_and_emits_no_lifecycle_event() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let old =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let old_id = Uuid::parse_str(&old.id).unwrap();
    backend.kill(old_id).unwrap();
    app.state::<crate::session::selection::SelectionCoordinator>()
        .container_lifecycle_sender()
        .route_lost(old_id, 23)
        .await
        .expect("reconcile dormant route");
    let events = capture_session_lifecycle(&app);
    backend.fail_spawn(2);
    let settings = app.state::<crate::config::settings::SettingsState>();

    super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(false),
        true,
    )
    .await
    .expect_err("dormant replacement spawn must fail");
    let retained = session_mgr
        .read()
        .await
        .get_session(old_id)
        .await
        .expect("dormant old row retained");
    assert_eq!(retained.status, SessionStatus::Exited(23));
    let selection = session_mgr.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(old_id));
    assert_eq!(selection.status(), Some(&SessionStatus::Exited(23)));
    assert!(events.try_recv().is_err());
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn create_session_inner_rolls_back_pre_created_session_on_spawn_error() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let err = super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        // #973 - headless caller: no terminal to measure, keep 120x30.
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("spawn should fail");

    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend.killed().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reserved rollback kills the failed spawn exactly once");
    assert_eq!(backend.killed(), backend.spawned());
    assert_eq!(backend.killed().len(), 1);
}

/// #973 - THE REGRESSION. Red against main, which hardcodes `cols: 120, rows: 30`.
///
/// AC used to open every ConPTY at 120x30 and let the frontend correct it 300-500 ms
/// later. That correction lands inside a coding agent's TUI startup, and a resize there
/// makes Codex redraw its still-empty viewport and lose the wakeup for the content that
/// becomes ready straight after: a blank terminal, alive, until any key is pressed.
///
/// Measured outside AC, on a bare ConPTY: opened at 120x30 and then resized in the
/// window, Codex comes up blank 8 times in 10. Opened at the size the view actually
/// wants, and never resized, 0 times in 10.
///
/// This pins the plumbing: the size the caller supplies is the size the PTY is opened
/// at. It does not, and cannot, prove Codex stops hanging - that needs a real child in a
/// real ConPTY inside a ~100 ms race. The harness is the proof of that.
#[tokio::test]
async fn privileged_preheld_target_gate_does_not_reenter_and_rechecks_pending_or_live() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
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
    let target =
        crate::config::teams::verify_pty_input_replica_cwd(std::path::Path::new(&first_cwd))
            .unwrap();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let gate = state.target_gate.as_ref().unwrap().clone();
    let stripe = gate
        .acquire_target_lock(&target.canonical_fqn)
        .await
        .unwrap();
    let exact = gate.acquire_exact(&target.canonical_fqn).await;
    let ownership = gate
        .target_ownership(&target.canonical_fqn, &stripe, &exact)
        .unwrap();

    let pending = pty_mgr
        .lock()
        .unwrap()
        .mark_spawning(&first_cwd, "pending race");
    let pending_error = super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd.clone(),
        Some("privileged target".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap_err();
    assert_eq!(pending_error, "sessionRace");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 0);
    drop(pending);

    let created = super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd.clone(),
        Some("privileged target".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap();
    let super::PtyTargetCreateStep::Finalized(created) = created else {
        panic!("Foreign create must actually finalize");
    };
    assert_eq!(created.working_directory, first_cwd);
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

    let live_error = super::create_session_inner_with_pty_target_ownership(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        first_cwd,
        Some("stale background create".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None,
        CreateSelectionIntent::Background,
        &ownership,
    )
    .await
    .unwrap_err();
    assert_eq!(live_error, "sessionRace");
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ordinary_strict_create_is_fail_soft_when_pty_store_is_error_or_missing() {
    for store_state in [SessionTestStoreState::Error, SessionTestStoreState::Missing] {
        let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app_with_store(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
            store_state,
        );

        let created = create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &first_cwd,
            CreateSelectionIntent::User,
        )
        .await;

        assert!(
            created.is_ok(),
            "ordinary create must not depend on the specialized PTY store: {created:?}"
        );
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

        let restore =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let restored = super::create_session_inner_for_restore(
            &restore,
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            second_cwd,
            Some("restore target".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(
            restored.is_ok(),
            "startup restore must not depend on the specialized PTY store: {restored:?}"
        );
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
        close_test_coordinator(&app).await;
    }
}

#[tokio::test]
async fn sequential_user_same_target_creates_remain_compatible() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
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
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    create_target_for_test(
        &app,
        &session_mgr,
        &pty_mgr,
        &first_cwd,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 2);
}

#[tokio::test]
async fn different_strict_targets_create_concurrently() {
    let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    backend.gate_spawn(1, started_tx, release_rx);
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let first_handle = app.handle().clone();
    let first_sessions = Arc::clone(&session_mgr);
    let first_pty = Arc::clone(&pty_mgr);
    let first_task = tokio::spawn(async move {
        super::create_session_inner(
            &first_handle,
            &first_sessions,
            &first_pty,
            "codex".to_string(),
            Vec::new(),
            first_cwd,
            Some("first target".to_string()),
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
    });
    started_rx.await.unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &second_cwd,
            CreateSelectionIntent::User,
        ),
    )
    .await
    .expect("a different target must not wait for the first exact gate")
    .unwrap();
    assert_eq!(second.working_directory, second_cwd);
    release_tx.send(()).unwrap();
    first_task.await.unwrap().unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_same_target_creates_serialize_on_the_exact_gate() {
    let (_fixture, first_cwd, _) = strict_target_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    // Gate the first spawn so the winning create holds the exact target gate.
    backend.gate_spawn(1, started_tx, release_rx);
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    // Task A wins the exact gate for the target and blocks inside its spawn.
    let a_task = {
        let handle = app.handle().clone();
        let sessions = Arc::clone(&session_mgr);
        let pty = Arc::clone(&pty_mgr);
        let cwd = first_cwd.clone();
        tokio::spawn(async move {
            super::create_session_inner(
                &handle,
                &sessions,
                &pty,
                "codex".to_string(),
                Vec::new(),
                cwd,
                Some("first same-target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                CreateSelectionIntent::Background,
            )
            .await
        })
    };
    started_rx.await.unwrap();

    // Task B is a concurrent background create for the SAME target. It must
    // block on the exact gate that A holds, not race into the missing-target
    // window beside A.
    let b_task = {
        let handle = app.handle().clone();
        let sessions = Arc::clone(&session_mgr);
        let pty = Arc::clone(&pty_mgr);
        let cwd = first_cwd.clone();
        tokio::spawn(async move {
            super::create_session_inner(
                &handle,
                &sessions,
                &pty,
                "codex".to_string(),
                Vec::new(),
                cwd,
                Some("second same-target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                CreateSelectionIntent::Background,
            )
            .await
        })
    };

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !b_task.is_finished(),
        "a second same-target create must block on the exact gate while the first holds it"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "no second spawn may start while the exact target gate is held"
    );

    // Release A; it finalizes a live session and drops the gate. B then wins the
    // gate, rechecks live/pending state, and refuses to spawn a duplicate.
    release_tx.send(()).unwrap();
    let created = a_task.await.unwrap().unwrap();
    assert_eq!(created.working_directory, first_cwd);
    let raced = b_task.await.unwrap().unwrap_err();
    assert_eq!(
        raced, "sessionRace",
        "the serialized loser must observe the now-live target and not create beside it"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "the serialized loser never spawned a duplicate"
    );
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
}

#[tokio::test]
async fn create_session_opens_the_pty_at_the_size_the_view_supplied() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        Some(crate::pty::backend::PtyViewport::from_fit(74, 23)),
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(74, 23)],
        "the PTY must be opened at the size the view fitted to, not at 120x30: \
             opening at the wrong size is what forces the startup resize that loses \
             Codex's first render (#973)"
    );
}

/// #973 - backward compatibility. Every headless caller (startup restore, the delivery
/// loop, the phone mailbox, the CLI, tests) has no terminal to measure and must keep
/// AC's historical 120x30. Those sessions are never attached to a view, so nothing
/// resizes them, and none of them ever hit this bug.
#[tokio::test]
async fn create_session_without_a_view_keeps_the_historical_120x30() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        None, // no view
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(120, 30)],
        "a caller with no view must behave exactly as it did before #973"
    );
}

/// #973 - opening a 0-column ConPTY would be worse than the bug we are fixing, so a
/// degenerate fitted size must fall back rather than be honoured.
///
/// It does not come from xterm: `fit()` clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1. It is
/// guarded because this is a `u16` boundary that anything upstream can hand a 0 to, and
/// because the cost of being wrong is a terminal with no screen at all.
#[tokio::test]
async fn a_degenerate_fitted_size_falls_back_instead_of_opening_a_zero_column_pty() {
    let temp = tempfile::tempdir().unwrap();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let _ = super::create_session_inner(
        &app_handle,
        &session_mgr,
        &pty_mgr,
        "missing-ac-test-command".to_string(),
        Vec::new(),
        temp.path().to_string_lossy().to_string(),
        None,
        None,
        None,
        true,
        Vec::new(),
        true,
        None,
        None,
        Some(crate::pty::backend::PtyViewport::from_fit(0, 0)),
        CreateSelectionIntent::User,
    )
    .await;

    assert_eq!(
        backend.sizes(),
        vec![(120, 30)],
        "a 0x0 fit must fall back to the default, never open a 0-column ConPTY"
    );
}

/// #2411 T1: the coordinator-create helper seeds the badge and clears both
/// close markers. Deleting any of its three clock calls fails this test.
#[test]
fn note_coordinator_create_on_app_seeds_and_clears_both_markers() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(Arc::clone(&clocks));
    let handle = app.handle().clone();
    let cwd = "C:/ac-test/project/.ac/wg-2411-dev-team/__agent_tech-lead";
    let fqn = crate::config::teams::agent_fqn_from_path(cwd);

    // (c) absent fqn -> badge seeded.
    assert!(clocks.lock().unwrap().last_user_message_at(&fqn).is_none());
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert!(clocks.lock().unwrap().last_user_message_at(&fqn).is_some());

    // (a) auto-closed marker cleared.
    assert!(clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&fqn, chrono::Utc::now()));
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);

    // (b) manual-close marker cleared.
    assert!(clocks
        .lock()
        .unwrap()
        .mark_manually_closed(&fqn, chrono::Utc::now()));
    crate::commands::session::note_coordinator_create_on_app(&handle, cwd)
        .expect("clocks managed")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
}

// ---- #2413: close markers survive a failed coordinator create ----

const MARKER_CWD: &str = "C:/ac-test/project/.ac/wg-2413-dev-team/__agent_tech-lead";

fn marker_ts(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp")
}

/// Mock app with a clocks store and a recorder for both close-marker events.
fn marker_unit_app() -> (
    tauri::App<tauri::test::MockRuntime>,
    crate::config::coordinator_clocks::CoordinatorClocksState,
    std::sync::mpsc::Receiver<(String, String)>,
) {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    app.manage(Arc::clone(&clocks));
    let events = marker_events(&app);
    (app, clocks, events)
}

/// Records `(event, value)` for both close-marker events; `value` is the
/// timestamp field (`null` for a clear).
fn marker_events(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<(String, String)> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    for (event_name, field) in [
        ("coordinator_auto_close_changed", "autoClosedAt"),
        ("coordinator_manual_close_changed", "manuallyClosedAt"),
    ] {
        let sender = sender.clone();
        app.listen_any(event_name, move |event| {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload()).expect("event payload json");
            let value = match &payload[field] {
                serde_json::Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            let _ = sender.send((event_name.to_string(), value));
        });
    }
    receiver
}

fn drain(events: &std::sync::mpsc::Receiver<(String, String)>) -> Vec<(String, String)> {
    events.try_iter().collect()
}

fn auto_event(value: &str) -> (String, String) {
    (
        "coordinator_auto_close_changed".to_string(),
        value.to_string(),
    )
}

fn manual_event(value: &str) -> (String, String) {
    (
        "coordinator_manual_close_changed".to_string(),
        value.to_string(),
    )
}

/// #2413 T1
#[test]
fn restore_close_markers_puts_back_original_auto_timestamp() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let k = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    k.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));
    assert_eq!(
        drain(&events),
        vec![auto_event("null"), auto_event(&t.to_rfc3339())]
    );
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T2
#[test]
fn restore_close_markers_puts_back_original_manual_timestamp() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_manually_closed(&fqn, t));
    let k = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
    k.failed();
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), Some(t));
    assert_eq!(
        drain(&events),
        vec![manual_event("null"), manual_event(&t.to_rfc3339())]
    );
}

/// #2413 T3: a manual close during the failing create wins over the auto restore.
#[test]
fn restore_close_markers_manual_wins() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    let manual = marker_ts(50);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let k = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    assert!(clocks.lock().unwrap().mark_manually_closed(&fqn, manual));
    k.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(
        clocks.lock().unwrap().manually_closed_at(&fqn),
        Some(manual)
    );
    assert_eq!(drain(&events), vec![auto_event("null")]);
}

/// #2413 T4
#[test]
fn restore_close_markers_noop_when_nothing_cleared() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    super::note_coordinator_create_on_app(app.handle(), MARKER_CWD)
        .expect("ticket")
        .failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
    assert!(drain(&events).is_empty());
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T4
#[test]
fn restore_close_markers_succeeded_never_restores() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
    super::note_coordinator_create_on_app(app.handle(), MARKER_CWD)
        .expect("ticket")
        .succeeded();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(drain(&events), vec![auto_event("null")]);
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T12: a ticket dropped without an end counts as a failure.
#[test]
fn dropped_ticket_restores_marker() {
    let (app, clocks, _events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    drop(super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket"));
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));
    assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
}

/// #2413 T13: once the spawn fence is set, no end restores anything.
#[test]
fn fenced_ticket_never_restores() {
    for use_drop in [false, true] {
        let (app, clocks, events) = marker_unit_app();
        let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
        assert!(clocks
            .lock()
            .unwrap()
            .mark_manually_closed(&fqn, marker_ts(5)));
        // `mark_manually_closed` clears auto, so both markers cannot be set at
        // once: a first create escrows manual, a second one escrows auto.
        let first =
            super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
        assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
        let mut k =
            super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
        drop(first); // hands off: k is still running
        k.session_may_exist = true;
        if use_drop {
            drop(k);
        } else {
            k.failed();
        }
        assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
        assert_eq!(clocks.lock().unwrap().manually_closed_at(&fqn), None);
        assert!(
            drain(&events).iter().all(|(_, v)| v == "null"),
            "no timestamp event"
        );
        assert!(!clocks.lock().unwrap().has_create_ledger(&fqn));
    }

    // Overlap: an unfenced ticket that fails after a fenced end restores nothing.
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, marker_ts(0)));
    let a = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    let mut k = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    k.session_may_exist = true;
    k.failed();
    a.failed();
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    assert_eq!(drain(&events), vec![auto_event("null")]);
}

/// #2413 grinch step-9: a failed create's restore paused before its emit must
/// not deliver its timestamp after a newer create's clear. The UI-visible
/// state (last event) must match the clock.
#[test]
fn restore_emit_never_lands_after_newer_create_clear() {
    let (app, clocks, events) = marker_unit_app();
    let fqn = crate::config::teams::agent_fqn_from_path(MARKER_CWD);
    let t = marker_ts(0);
    assert!(clocks.lock().unwrap().mark_auto_closed(&fqn, t));
    let failing = super::note_coordinator_create_on_app(app.handle(), MARKER_CWD).expect("ticket");
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    super::restore_emit_seam::install(&fqn, reached_tx, release_rx);

    let restorer = std::thread::spawn(move || failing.failed());
    reached_rx.recv().expect("restore paused before its emit");
    // The clock holds the restored marker; its event is not out yet.
    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), Some(t));

    let handle = app.handle().clone();
    let (newer_done_tx, newer_done_rx) = std::sync::mpsc::channel();
    let newer = std::thread::spawn(move || {
        super::note_coordinator_create_on_app(&handle, MARKER_CWD)
            .expect("ticket")
            .succeeded();
        let _ = newer_done_tx.send(());
    });
    // Without ordering the newer create finishes while the restore is
    // paused; with it, it waits. Either way, release afterwards.
    let _ = newer_done_rx.recv_timeout(Duration::from_millis(200));
    release_tx.send(()).unwrap();
    restorer.join().unwrap();
    newer.join().unwrap();

    assert_eq!(clocks.lock().unwrap().auto_closed_at(&fqn), None);
    let auto_events: Vec<_> = drain(&events)
        .into_iter()
        .filter(|(name, _)| name == "coordinator_auto_close_changed")
        .collect();
    assert_eq!(
        auto_events,
        vec![
            auto_event("null"),
            auto_event(&t.to_rfc3339()),
            auto_event("null")
        ],
        "last event must match the cleared clock"
    );
}

/// #2413 T5: the spawn fence is set before the spawn await and cleared only
/// on a spawn `Err` without a launch.
#[test]
fn create_session_inner_fences_marker_ticket_around_spawn() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let fence = "t.session_may_exist=true;";
    let spawn = "letspawn_result=PtyManager::spawn(";
    let unfence = "if!launch_witness.launched(){";
    assert_eq!(normalized.matches(fence).count(), 1);
    assert_eq!(normalized.matches(spawn).count(), 1);
    assert_eq!(normalized.matches(unfence).count(), 1);
    let fence_at = normalized.find(fence).unwrap();
    let spawn_at = normalized.find(spawn).unwrap();
    let unfence_at = normalized.find(unfence).unwrap();
    assert!(fence_at < spawn_at, "fence must precede the spawn await");
    assert!(spawn_at < unfence_at, "unfence only after the spawn result");
    assert_eq!(normalized.matches("has_backend_session").count(), 0);
    assert!(normalized
        .contains("letpending_result=mgr.create_pending_session_with_binding_and_carrier_permit("));
}

struct MarkerCreateFixture {
    _temp: tempfile::TempDir,
    cwd: String,
    fqn: String,
    app: tauri::App<tauri::test::MockRuntime>,
    clocks: crate::config::coordinator_clocks::CoordinatorClocksState,
    events: std::sync::mpsc::Receiver<(String, String)>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
}

impl MarkerCreateFixture {
    fn new(backend: Arc<dyn crate::pty::backend::PtyBackend>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd =
            crate::path_utils::normalize_windows_verbatim_path(&temp.path().to_string_lossy());
        let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
            Arc::new(Mutex::new(Default::default()));
        app.manage(Arc::clone(&clocks));
        let events = marker_events(&app);
        super::coordinator_create_seam::force(&cwd);
        Self {
            _temp: temp,
            cwd,
            fqn,
            app,
            clocks,
            events,
            session_mgr,
            pty_mgr,
        }
    }

    fn create(&self) -> impl std::future::Future<Output = Result<SessionInfo, String>> + '_ {
        super::create_session_inner(
            self.app.handle(),
            &self.session_mgr,
            &self.pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            self.cwd.clone(),
            None,
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
    }

    fn auto(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.clocks.lock().unwrap().auto_closed_at(&self.fqn)
    }

    fn ledger(&self) -> bool {
        self.clocks.lock().unwrap().has_create_ledger(&self.fqn)
    }
}

impl Drop for MarkerCreateFixture {
    fn drop(&mut self) {
        super::coordinator_create_seam::unforce(&self.cwd);
    }
}

/// #2413 T6
#[tokio::test]
async fn create_session_inner_restores_auto_closed_marker_on_spawn_failure() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_auto_closed(&f.fqn, t));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert_eq!(f.auto(), Some(t));
    assert_eq!(
        drain(&f.events),
        vec![auto_event("null"), auto_event(&t.to_rfc3339())]
    );
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2413 T7
#[tokio::test]
async fn create_session_inner_restores_manual_closed_marker_on_spawn_failure() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_manually_closed(&f.fqn, t));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic spawn failure"), "{err}");
    assert_eq!(f.clocks.lock().unwrap().manually_closed_at(&f.fqn), Some(t));
    assert_eq!(
        drain(&f.events),
        vec![manual_event("null"), manual_event(&t.to_rfc3339())]
    );
    close_test_coordinator(&f.app).await;
}

/// #2413 T8: a newer overlapping create that succeeded wins; no stale restore.
#[tokio::test]
async fn create_session_inner_failed_create_does_not_restore_after_newer_create() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let barrier = super::seed_race_barriers::install_before_project_gate(&f.cwd);
    let driver = async {
        barrier.reached.notified().await;
        assert_eq!(f.auto(), None);
        super::note_coordinator_create_on_app(f.app.handle(), &f.cwd)
            .expect("ticket")
            .succeeded();
        barrier.release.notify_one();
    };
    let (result, ()) = tokio::join!(f.create(), driver);
    result.expect_err("spawn should fail");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2413 T11: overlapping creates that all fail restore the marker exactly once.
#[tokio::test]
async fn create_session_inner_two_failed_overlapping_creates_restore_marker() {
    let f = MarkerCreateFixture::new(Arc::new(FailingSpawnBackend::default()));
    let t = marker_ts(0);
    assert!(f.clocks.lock().unwrap().mark_auto_closed(&f.fqn, t));
    let barrier = super::seed_race_barriers::install_before_project_gate(&f.cwd);
    let driver = async {
        barrier.reached.notified().await;
        super::note_coordinator_create_on_app(f.app.handle(), &f.cwd)
            .expect("ticket")
            .failed();
        // Handed off to the running create: nothing restored yet.
        assert_eq!(f.auto(), None);
        assert_eq!(drain(&f.events), vec![auto_event("null")]);
        barrier.release.notify_one();
    };
    let (result, ()) = tokio::join!(f.create(), driver);
    result.expect_err("spawn should fail");
    assert_eq!(f.auto(), Some(t));
    assert_eq!(drain(&f.events), vec![auto_event(&t.to_rfc3339())]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// Test backend for #2413 T14/T15.
/// `block`: `spawn` records the id (live) and waits for `release` after signaling `entered`.
/// Otherwise: `spawn` marks the launch witness, then fails with no session.
#[derive(Default)]
struct MarkerSpawnBackend {
    block: bool,
    block_only_first: bool,
    spawn_count: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    live: Mutex<Vec<Uuid>>,
}

impl crate::pty::backend::PtyBackend for MarkerSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            spec.launch_witness.mark();
            let index = self.spawn_count.fetch_add(1, Ordering::SeqCst);
            if self.block && (!self.block_only_first || index == 0) {
                self.live.lock().unwrap().push(spec.id);
                self.entered.notify_one();
                self.release.notified().await;
            }
            Err(crate::errors::AppError::PtyError(
                "synthetic post-launch failure".to_string(),
            ))
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
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

/// #2413 T14: a create cancelled while spawn runs keeps the markers cleared.
#[tokio::test]
async fn create_session_inner_keeps_markers_cleared_when_cancelled_after_spawn() {
    // Variant 1: the create future is dropped mid-spawn.
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        ..Default::default()
    });
    let f = MarkerCreateFixture::new(backend.clone());
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let mut create = Box::pin(f.create());
    tokio::select! {
        _ = backend.entered.notified() => {}
        _ = &mut create => panic!("create must stay parked in spawn"),
    }
    drop(create);
    assert_eq!(f.auto(), None);
    assert!(f.ledger(), "accepted core still owns the in-flight spawn");
    backend.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.ledger() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retained create observes its real spawn error");
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    close_test_coordinator(&f.app).await;

    // Variant 2: the selection coordinator shuts down mid-spawn.
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        ..Default::default()
    });
    let f = MarkerCreateFixture::new(backend.clone());
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let shutdown = async {
        backend.entered.notified().await;
        f.app
            .state::<crate::session::selection::SelectionCoordinator>()
            .shutdown_token()
            .cancel();
        assert!(
            f.ledger(),
            "shutdown does not cancel the accepted backend effect"
        );
        backend.release.notify_one();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(f.create(), shutdown)
    })
    .await
    .expect("create settles after backend result despite shutdown");
    result.expect_err("shutdown ends the create");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

#[tokio::test]
async fn foreign_common_create_second_progresses_while_first_core_retained_after_caller_loss() {
    let backend = Arc::new(MarkerSpawnBackend {
        block: true,
        block_only_first: true,
        ..Default::default()
    });
    let fixture = MarkerCreateFixture::new(backend.clone());
    // This is the existing non-replica common-create path. No legacy
    // replica target stripe is bypassed to manufacture concurrent progress.
    assert!(
        crate::config::teams::pty_input_create_gate_key_from_cwd(std::path::Path::new(
            &fixture.cwd
        ))
        .unwrap()
        .is_none()
    );
    let mut first = Box::pin(fixture.create());
    tokio::select! {
        _ = backend.entered.notified() => {},
        _ = &mut first => panic!("first real core must be parked in backend spawn"),
    }
    drop(first);
    assert!(fixture.ledger(), "first accepted core remains retained");
    let second = tokio::time::timeout(Duration::from_secs(10), fixture.create())
        .await
        .expect(
            "second Foreign create must reach its actual backend result before first is released",
        );
    assert!(second
        .unwrap_err()
        .contains("synthetic post-launch failure"));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
    assert!(
        fixture.ledger(),
        "second completion cannot drop first create's owner"
    );
    backend.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.ledger() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first retained core eventually observes its actual result");
    close_test_coordinator(&fixture.app).await;
}

#[tokio::test]
async fn failed_tracker_foreign_common_create_keeps_real_legacy_backend_result() {
    let backend = Arc::new(MarkerSpawnBackend::default());
    let fixture = MarkerCreateFixture::new(backend.clone());
    let tracker = fixture.pty_mgr.lock().unwrap().response_close_state();
    tracker.fail_closed();
    let result = tokio::time::timeout(Duration::from_secs(10), fixture.create())
        .await
        .expect("E2 Foreign/nonClosing must reach real common-create completion");
    assert!(result
        .unwrap_err()
        .contains("synthetic post-launch failure"));
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    assert!(!fixture.ledger());
    assert!(
        matches!(
            tracker.reserve_create_owner(
                "still-failed",
                Uuid::new_v4(),
                crate::pty::manager::CreateClassification::Foreign {
                    publication_epoch: 0
                }
            ),
            Err(crate::pty::manager::ResponseCloseError::Unavailable)
        ),
        "ordinary create never resets or substitutes the failed tracker"
    );
    close_test_coordinator(&fixture.app).await;
}

/// #2413 T15: a spawn `Err` after the backend launched keeps the markers cleared,
/// even though the backend reports no session.
#[tokio::test]
async fn create_session_inner_keeps_markers_cleared_when_spawn_fails_after_launch() {
    let f = MarkerCreateFixture::new(Arc::new(MarkerSpawnBackend::default()));
    assert!(f
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(&f.fqn, marker_ts(0)));
    let err = f.create().await.expect_err("spawn should fail");
    assert!(err.contains("synthetic post-launch failure"), "{err}");
    assert_eq!(f.auto(), None);
    assert_eq!(drain(&f.events), vec![auto_event("null")]);
    assert!(!f.ledger());
    close_test_coordinator(&f.app).await;
}

/// #2411 T1b: `create_session_inner_impl` calls the coordinator-create helper
/// exactly once (source guard, same pattern as the archive-gate test below).
#[test]
fn create_session_inner_calls_note_coordinator_create_once() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    assert_eq!(
        normalized
            .matches("note_coordinator_create_on_app(app,&cwd)")
            .count(),
        1,
        "create_session_inner_impl must call note_coordinator_create_on_app once"
    );
}

#[test]
fn create_session_inner_keeps_both_archive_activation_gates() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let gate_call = "enforce_owned_unarchived_for_spawn(app,&cwd,&session_label,&create_owner,&create_manager,lifecycle_domain,inline_transaction,)";
    let count = normalized.matches(gate_call).count();

    assert_eq!(
        count, 2,
        "create_session_inner must keep both archive activation gates"
    );
}

// T16 (#1172 D5), rescoped by #1175. READ THIS BEFORE TRUSTING IT.
//
// This is a SOURCE-LEVEL INVENTORY over `session.rs` alone. It does NOT prove
// that a resume never rotates, and it does not close the class of changes that
// retarget the rotation. Three measured facts bound it:
//
//   - A cross-file alias (`pub(crate) use ... as <alias>;` in `config/mod.rs`
//     plus an ungated call to the alias here) keeps assertions 1 to 3 intact
//     while a resume rotates. Recorded in the issue.
//   - A cross-file CALL added in `lib.rs` before the restore at `:2350` is
//     invisible to every assertion here and to both behavioral tests below.
//   - `skip_auto_resume |= is_coordinator;` inserted immediately above the
//     binding leaves ALL FIVE assertions and BOTH behavioral tests green while
//     a coordinator resume rotates. Measured; plan 2.8.
//
// What it DOES enforce, and what nothing else in this suite can:
//   - assertions 1 to 3: `rotate_origin_memory_at_spawn` is REFERENCED exactly
//     twice in this file, once in the comment and once in the gated call. A
//     third occurrence is an UNBUDGETED REFERENCE; it need not be a call. This
//     is what catches an ungated second call added on a launch branch the
//     behavioral tests do not drive (`execute_restart_transaction` at `:3897`,
//     the Root Agent launch at `:4703`).
//   - assertions 4 and 5: the name `start_fresh` is bound exactly once, and its
//     right-hand side is exactly the resume flag. That is a complete statement
//     about ONE LINE. It says nothing about the value flowing into that line,
//     which is the residual named above and in plan D5.
//
// The normative property is guarded behaviorally by
// `a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does`
// and `create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one`,
// for the entry points and argument shapes those tests drive and no further.
// Plan section 4.5 enumerates what neither side covers.
//
// One invisible dependency, already satisfied: the split needle below uses
// `\n`. This repository sets `core.autocrlf=true` on Windows checkouts, and
// `.gitattributes` `*.rs text eol=lf` is what keeps `session.rs` LF on disk
// (measured: 0 CRLF, 9344 LF). If that attribute were ever dropped, this test
// fails LOUD rather than silent: assertion 1 would count 2, because the needle
// also appears as this test's own `let gated_call = "..."` literal.
#[test]
fn rotate_origin_memory_at_spawn_has_one_gated_call_in_session_rs() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let gated_call = "ifstart_fresh&&context_result.is_ok(){crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);}";

    assert_eq!(
        normalized.matches(gated_call).count(),
        1,
        "#1172 D5: the memory rotation must stay behind `start_fresh && context_result.is_ok()`. \
             This pins the gate's exact spelling at the single call site in this file; it does not \
             prove the resume property, and the behavioral tests named above prove it only for the \
             entry points they drive."
    );
    // What assertions 2 and 3 add to assertion 1, stated as the bounded
    // property they actually enforce.
    //
    // Assertion 1 pins one exact spelling of the gated call, so a second
    // call spelled differently would not match its needle: a different
    // argument, the path wrapped in parens so it reads `..._at_spawn)(`, or
    // a block comment between the identifier and the paren. Assertions 2
    // and 3 reach those by counting the BARE identifier rather than an
    // invocation shape, and by pinning the one legitimate occurrence that
    // is not a call, the comment 5.4 requires above the gate.
    //
    // The property they enforce, and nothing broader: within `session.rs`
    // the exact identifier `rotate_origin_memory_at_spawn` occurs exactly
    // twice, once in that comment and once in the call assertion 1 already
    // pinned. A third occurrence is unbudgeted.
    //
    // That is a count of one identifier in one file. It does NOT establish
    // that every route to the rotation is accounted for. A call reached
    // through a cross-file alias, and a call added in another file, both
    // leave this count at two; both are recorded in the header above, and
    // the second is plan 9.2's M9, measured green against all five
    // assertions here and both behavioral tests below.
    let comment_mention = "`rotate_origin_memory_at_spawn`returns`()`";
    assert_eq!(
        normalized.matches(comment_mention).count(),
        1,
        "#1172 D5: the comment above the gate must keep naming the rotation exactly once; \
             it is the one non-call mention the reference budget below accounts for."
    );
    assert_eq!(
        normalized.matches("rotate_origin_memory_at_spawn").count(),
        2,
        "#1172 D5: within THIS FILE the rotation entry point must be REFERENCED exactly twice - \
             once in the comment above the gate, once in the gated call itself. A third occurrence \
             is unbudgeted: it may be a second call site, an import, or any other reference, and \
             each of those is a change this inventory deliberately refuses to absorb silently. A \
             cross-file alias evades this count entirely; see the header comment."
    );
    // #1175 D2 part 4. The gate's INPUT, not just its shape. Bounded claim: this
    // pins ONE LINE. It cannot see how `skip_auto_resume` got its value; see the
    // header and plan D5 residual 3.
    assert_eq!(
        normalized
            .matches("letstart_fresh=skip_auto_resume;")
            .count(),
        1,
        "#1175: `start_fresh` must be bound to the resume flag ALONE. Appending `|| <extra>` \
             here makes a RESUME rotate on the production launches that supply the extra input. \
             Measured (plan 2.7): `|| pending_start_fresh.is_some()` reds this assertion AND the \
             production-shaped restore test; the `|| is_coordinator` variant reds only this one, \
             because no cwd in this test binary can be a coordinator."
    );
    // #1175 round 2. Assertion 4 alone is defeated by ordinary shadowing:
    // `let start_fresh = skip_auto_resume; let start_fresh = start_fresh || X;`
    // preserves its needle exactly. MEASURED (plan 2.8): this assertion is the
    // only thing in the suite that reds on that form.
    assert_eq!(
        normalized.matches("letstart_fresh=").count(),
        1,
        "#1175: `start_fresh` must be bound EXACTLY ONCE. A second binding shadows the first \
             and can compose any additional input into the rotation decision while assertion 4's \
             needle stays intact."
    );
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
    let spawn =
        super::build_configured_agent_spawn_for_cwd(&settings, "codex", &resumed.replica_cwd, None)
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
    let restored = super::create_session_inner_for_restore(
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

    let created = super::create_session_inner(
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

    let created = super::create_session_inner(
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

    let reopened = super::create_session_inner(
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
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let first_gate = normalized
            .find("enforce_owned_unarchived_for_spawn(app,&cwd,&session_label,&create_owner,&create_manager,lifecycle_domain,inline_transaction,).await?;")
            .expect("first archive gate");
    let mark = normalized
        .find("letspawn_mark={")
        .expect("spawn mark immediately before pending-session creation");
    let pending_create = normalized
        .find("letpending_result=mgr.create_pending_session_with_binding_and_carrier_permit(")
        .expect("pending-session creation");
    let spawn = normalized
        .find("letspawn_result=PtyManager::spawn(pty_mgr,session.backend_kind,spawn_spec).await;")
        .expect("spawn call");
    let drop_mark = normalized
        .find("drop(spawn_mark);")
        .expect("spawn mark drop");
    let spawn_error = normalized
        .find("ifletErr(e)=spawn_result{")
        .expect("spawn error handling");

    assert!(
        first_gate < mark,
        "spawn mark must be created only after pre-create validation"
    );
    assert!(
        mark < pending_create,
        "spawn mark must precede pending-session creation"
    );
    assert!(
        pending_create < spawn,
        "pending-session creation must run before PTY spawn"
    );
    assert!(
        spawn < drop_mark,
        "spawn mark must stay live while spawn awaits"
    );
    assert!(
        drop_mark < spawn_error,
        "spawn mark must drop immediately after spawn returns"
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
            super::create_session_inner(
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
            super::create_session_inner(
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

#[test]
fn resolve_root_agent_command_prefers_valid_explicit_agent() {
    let settings = test_settings();

    let (shell, args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("codex"), Some("claude")).unwrap();

    assert_eq!(shell, "codex");
    assert!(args.is_empty());
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_uses_last_coding_agent_when_explicit_missing() {
    let settings = test_settings();

    let (shell, _args, agent_id, label) =
        resolve_root_agent_command(&settings, None, Some("codex")).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_falls_back_to_first_configured_agent() {
    let settings = test_settings();

    let (shell, _args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap();

    assert_eq!(shell, "claude");
    assert_eq!(agent_id.as_deref(), Some("claude"));
    assert_eq!(label.as_deref(), Some("Claude Code"));
}

#[test]
fn resolve_root_agent_command_rejects_default_shell_without_agents() {
    let mut settings = AppSettings {
        default_shell: "pwsh".to_string(),
        default_shell_args: vec!["-NoLogo".to_string()],
        ..AppSettings::default()
    };
    settings.agents.clear();

    let err = resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap_err();

    assert!(err.contains("No resolvable coding agent"));
}

fn write_current_coding_agent(dir: &std::path::Path, agent_id: &str) {
    std::fs::write(
        dir.join("config.json"),
        format!(r#"{{"tooling":{{"currentCodingAgent":"{}"}}}}"#, agent_id),
    )
    .unwrap();
}

// #537 read-side: a freshly assigned currentCodingAgent must win over the
// agent the live session was launched with (stored_agent_id) when the restart
// carries no explicit agent (the plain restart button passes None).
#[test]
fn restart_honors_current_coding_agent_when_no_explicit_request() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "codex");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("codex"));
}

#[test]
fn restart_explicit_agent_overrides_current_coding_agent() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "codex");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        Some("claude"),
        Some("codex"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

#[test]
fn restart_ignores_unconfigured_current_coding_agent_and_keeps_stored() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();
    write_current_coding_agent(tmp.path(), "ghost");

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

// Root agents and plain terminals have no currentCodingAgent → behavior
// unchanged: fall straight through to the stored launch agent.
#[test]
fn restart_falls_back_to_stored_when_no_current_coding_agent() {
    let settings = test_settings();
    let tmp = tempfile::tempdir().unwrap();

    let selected = resolve_restart_selected_agent_id(
        &settings,
        &tmp.path().to_string_lossy(),
        None,
        Some("claude"),
    );

    assert_eq!(selected.as_deref(), Some("claude"));
}

#[test]
fn restart_requested_profile_prefers_explicit_then_stored() {
    assert_eq!(
        effective_restart_requested_profile(Some("C".to_string()), Some("B".to_string())),
        Some("C".to_string())
    );
    assert_eq!(
        effective_restart_requested_profile(None, Some("B".to_string())),
        Some("B".to_string())
    );
    assert_eq!(effective_restart_requested_profile(None, None), None);
}

#[test]
fn build_configured_agent_spawn_for_cwd_honors_requested_profile() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "C".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                // #597 - the cell holds params only; they append to the agent
                // base command (`codex`) to launch `codex --profile-c`.
                command: "--profile-c".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd, Some("C"))
        .unwrap()
        .unwrap();

    assert_eq!(spawn.profile_resolution.requested_profile, "C");
    assert_eq!(spawn.profile_resolution.effective_profile, "C");
    assert_eq!(spawn.shell_args, vec!["--profile-c".to_string()]);
}

// #592 - compute_profile_outdated integration: loaded hash vs current config.
fn make_info(
    cwd: &str,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
    loaded_hash: Option<String>,
) -> SessionInfo {
    SessionInfo {
        id: "00000000-0000-0000-0000-000000000000".to_string(),
        name: "s".to_string(),
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-06-21T00:00:00Z".to_string(),
        working_directory: cwd.to_string(),
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: agent_id.map(str::to_string),
        agent_label: None,
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".to_string(),
        agent_kind: None,
        requested_profile: requested_profile.map(str::to_string),
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: loaded_hash,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

#[test]
fn compute_profile_outdated_flips_when_effective_cell_changes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--v1".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    // #597 - the hash the session launched with: agent base + cell A params,
    // composed via the SAME helpers the spawn path uses. The codex borrow ends
    // with this block, before the get_mut mutation below.
    let loaded = {
        let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
        crate::config::agent_command::profile_content_hash(
            &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
            &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
        )
    };
    let mut info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

    // Config unchanged -> not outdated.
    assert!(!compute_profile_outdated(&settings, &info));

    // Edit the effective cell params -> outdated.
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--v2".to_string();
    assert!(compute_profile_outdated(&settings, &info));

    // A plain-shell session (no agent) never drifts.
    info.agent_id = None;
    assert!(!compute_profile_outdated(&settings, &info));
}

#[test]
fn compute_profile_outdated_flips_on_agent_base_command_edit() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();

    let mut settings = test_settings();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--v1".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    // #597 - stamp loaded from agent base `codex` + cell `--v1`.
    let loaded = {
        let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
        crate::config::agent_command::profile_content_hash(
            &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
            &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
        )
    };
    let info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

    // Base unchanged -> not outdated.
    assert!(!compute_profile_outdated(&settings, &info));

    // Edit the agent base command -> outdated (the base is now in the hash).
    settings
        .agents
        .iter_mut()
        .find(|a| a.id == "codex")
        .unwrap()
        .command = "codex-next".into();
    assert!(compute_profile_outdated(&settings, &info));
}

#[test]
fn resolve_agent_command_preserves_args() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex --yolo".to_string();

    let (shell, args, label) = resolve_agent_command("codex", &settings).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(args, vec!["--yolo".to_string()]);
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_agent_command_rejects_invalid_selected_command() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex \"unterminated".to_string();

    let err = resolve_agent_command("codex", &settings).unwrap_err();

    assert!(err.contains("selected agent 'codex'"));
    assert!(err.contains("agent id 'codex'"));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn resolve_root_agent_command_preserves_args() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex --yolo".to_string();

    let (shell, args, agent_id, label) =
        resolve_root_agent_command(&settings, Some("codex"), None).unwrap();

    assert_eq!(shell, "codex");
    assert_eq!(args, vec!["--yolo".to_string()]);
    assert_eq!(agent_id.as_deref(), Some("codex"));
    assert_eq!(label.as_deref(), Some("Codex"));
}

#[test]
fn resolve_root_agent_command_rejects_invalid_last_coding_agent_command() {
    let mut settings = test_settings();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .command = "codex \"unterminated".to_string();

    let err = resolve_root_agent_command(&settings, None, Some("codex")).unwrap_err();

    assert!(err.contains("root lastCodingAgent 'codex'"));
    assert!(err.contains("agent id 'codex'"));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn existing_root_classifier_discards_live_record_without_pty() {
    assert_eq!(
        classify_existing_root(&SessionStatus::Running, false),
        ExistingRootAction::DiscardMissingPty
    );
    assert_eq!(
        classify_existing_root(&SessionStatus::Active, true),
        ExistingRootAction::ReuseLive
    );
    assert_eq!(
        classify_existing_root(&SessionStatus::Exited(0), false),
        ExistingRootAction::WakeDormant
    );
}

#[tokio::test]
async fn dormant_root_command_resolution_failure_preserves_existing_session() {
    let session_mgr = SessionManager::new();
    let settings = AppSettings {
        agents: Vec::new(),
        ..AppSettings::default()
    };

    let dormant = {
        let session = session_mgr
            .create_session(
                "codex".to_string(),
                Vec::new(),
                "C:\\test\\ac-root-agent".to_string(),
                Some("codex".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("failed to create dormant root session");
        session_mgr.set_is_root_agent(session.id, true).await;
        session_mgr.mark_exited(session.id, 0).await;
        session
    };

    assert_eq!(
        classify_existing_root(&SessionStatus::Exited(0), false),
        ExistingRootAction::WakeDormant
    );

    let err = resolve_root_agent_command(&settings, Some("stale"), Some("also-stale"))
        .expect_err("replacement command should fail before destroying dormant root");

    assert!(err.contains("No resolvable coding agent"));
    let preserved = session_mgr.get_session(dormant.id).await;
    assert!(matches!(
        preserved.as_ref().map(|s| &s.status),
        Some(SessionStatus::Exited(0))
    ));
    assert!(preserved.is_some_and(|s| s.is_root_agent));
}

#[test]
fn pi_explicit_session_control_matches_only_decided_spellings() {
    for token in [
        "-c",
        "-r",
        "--continue",
        "--continue=true",
        "--resume",
        "--resume=id",
        "--session",
        "--session=id",
        "--session-id",
        "--session-id=id",
        "--fork",
        "--fork=id",
        "--no-session",
        "--no-session=true",
    ] {
        assert!(
            pi_has_explicit_session_control(&[token.to_string()]),
            "token={token:?}"
        );
    }
    for token in [
        "-cr",
        "--Continue",
        "--session-dir",
        "--session-dir=custom",
        "--session-directory",
        "--forked",
        "--no-sessions",
    ] {
        assert!(
            !pi_has_explicit_session_control(&[token.to_string()]),
            "token={token:?}"
        );
    }
}

#[test]
fn pi_non_conversation_policy_is_case_sensitive_and_position_aware() {
    for command in ["install", "remove", "uninstall", "update", "list", "config"] {
        assert!(pi_is_non_conversation_invocation(&[command.to_string()]));
        assert!(!pi_is_non_conversation_invocation(&[
            "--print".to_string(),
            command.to_string(),
        ]));
    }
    for flag in [
        "--help",
        "-h",
        "--version",
        "-v",
        "--export",
        "--export=file.jsonl",
        "--list-models",
        "--list-models=json",
    ] {
        assert!(pi_is_non_conversation_invocation(&[
            "--model".to_string(),
            "x".to_string(),
            flag.to_string(),
        ]));
    }
    for eligible in ["Install", "--Help", "--print", "-p", "--json", "--rpc"] {
        assert!(!pi_is_non_conversation_invocation(&[eligible.to_string()]));
    }
}

#[test]
fn inject_pi_resume_handles_direct_tokenized_and_embedded_commands() {
    let mut direct = vec!["--model".to_string(), "claude-sonnet".to_string()];
    assert!(inject_pi_resume("pi", &mut direct));
    assert_eq!(direct, vec!["--continue", "--model", "claude-sonnet"]);

    let mut tokenized = vec![
        "/C".to_string(),
        "pi.cmd".to_string(),
        "--session-dir".to_string(),
        r"C:\Pi State".to_string(),
    ];
    assert!(inject_pi_resume("cmd.exe", &mut tokenized));
    assert_eq!(
        tokenized,
        vec![
            "/C",
            "pi.cmd",
            "--continue",
            "--session-dir",
            r"C:\Pi State"
        ]
    );

    let original = r#"  "C:\Program Files\Pi\pi.cmd"  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
    let expected =
        r#"  "C:\Program Files\Pi\pi.cmd" --continue  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
    let mut embedded = vec!["/K".to_string(), original.to_string()];
    assert!(inject_pi_resume("cmd", &mut embedded));
    assert_eq!(embedded, vec!["/K", expected]);
    assert!(!inject_pi_resume("cmd", &mut embedded));
    assert_eq!(embedded, vec!["/K", expected]);
}

#[test]
fn inject_pi_resume_honors_every_selector_and_decoded_cmd_spelling() {
    for selector in [
        "-c",
        "-r",
        "--continue",
        "--continue=true",
        "--resume",
        "--resume=id",
        "--session",
        "--session=id",
        "--session-id",
        "--session-id=id",
        "--fork",
        "--fork=id",
        "--no-session",
        "--no-session=true",
    ] {
        let mut args = vec![selector.to_string(), "--model".to_string(), "x".to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "selector={selector:?}");
        assert_eq!(args, original);
    }

    for text in [r#"pi "--resume" --model x"#, "pi --res^ume --model x"] {
        let mut args = vec!["/C".to_string(), text.to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("cmd.exe", &mut args), "text={text:?}");
        assert_eq!(args, original);
    }
}

#[test]
fn inject_pi_resume_distinguishes_session_dir_and_prefix_names() {
    for initial in [
        vec!["--session-dir".to_string(), "custom".to_string()],
        vec!["--session-dir=custom".to_string()],
        vec!["--session-directory".to_string()],
    ] {
        let mut args = initial.clone();
        assert!(inject_pi_resume("pi", &mut args));
        assert_eq!(args.first().map(String::as_str), Some("--continue"));
        assert_eq!(&args[1..], initial.as_slice());
    }

    let mut explicit = vec!["--session".to_string(), "id".to_string()];
    let original = explicit.clone();
    assert!(!inject_pi_resume("pi", &mut explicit));
    assert_eq!(explicit, original);
}

#[test]
fn inject_pi_resume_preserves_non_conversation_invocations() {
    for command in ["install", "remove", "uninstall", "update", "list", "config"] {
        let mut args = vec![command.to_string(), "package".to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "command={command:?}");
        assert_eq!(args, original);
    }
    for flag in [
        "--help",
        "-h",
        "--version",
        "-v",
        "--export",
        "--export=file",
        "--list-models",
        "--list-models=json",
    ] {
        let mut args = vec!["--model".to_string(), "x".to_string(), flag.to_string()];
        let original = args.clone();
        assert!(!inject_pi_resume("pi", &mut args), "flag={flag:?}");
        assert_eq!(args, original);
    }

    for conversational in ["--print", "-p", "--json", "--rpc", "Install", "--Help"] {
        let mut args = vec![conversational.to_string()];
        assert!(inject_pi_resume("pi", &mut args));
        assert_eq!(args.first().map(String::as_str), Some("--continue"));
    }
}

#[test]
fn inject_pi_resume_inspects_only_the_first_cmd_segment() {
    let original = "pi --model x&&echo --resume";
    let expected = "pi --continue --model x&&echo --resume";
    let mut args = vec!["/C".to_string(), original.to_string()];
    assert!(inject_pi_resume("cmd.exe", &mut args));
    assert_eq!(args, vec!["/C", expected]);
}

#[test]
fn inject_pi_resume_leaves_malformed_unsupported_and_non_pi_inputs_unchanged() {
    let cases = [
        ("cmd.exe", vec!["/C".to_string(), "pi>out".to_string()]),
        (
            "cmd.exe",
            vec!["/C".to_string(), "pi \"unterminated".to_string()],
        ),
        (
            "cmd.exe",
            vec!["/C".to_string(), "npx".to_string(), "pi".to_string()],
        ),
        ("powershell.exe", vec!["pi".to_string()]),
    ];
    for (shell, mut args) in cases {
        let original = args.clone();
        assert!(!inject_pi_resume(shell, &mut args), "shell={shell:?}");
        assert_eq!(args, original);
    }
}

#[test]
fn maybe_inject_pi_resume_requires_kind_trusted_spawn_and_known_state() {
    let spawn = inert_pi_spawn();
    let base = vec!["--model".to_string(), "claude-sonnet".to_string()];

    let mut eligible = base.clone();
    assert!(maybe_inject_pi_resume(
        Some(CodingAgentKind::Pi),
        Some(&spawn),
        false,
        "pi",
        &mut eligible,
    ));
    assert_eq!(eligible, vec!["--continue", "--model", "claude-sonnet"]);

    for (kind, trusted, fresh) in [
        (Some(CodingAgentKind::Pi), None, false),
        (Some(CodingAgentKind::Pi), Some(&spawn), true),
        (Some(CodingAgentKind::Claude), Some(&spawn), false),
        (None, Some(&spawn), false),
    ] {
        let mut args = base.clone();
        assert!(!maybe_inject_pi_resume(
            kind, trusted, fresh, "pi", &mut args,
        ));
        assert_eq!(args, base);
    }
}

#[test]
fn pi_tuned_profile_flag_stays_false_while_direct_shell_capability_is_separate() {
    let mut settings = AppSettings::default();
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    let requested = crate::config::settings::resolve_auto_self_clear(&settings, "dev-rust", false);
    assert!(requested);
    // The tuned profile retains #1069's value. Launch gating uses #1059's
    // independent exact-stem direct-shell capability instead.
    assert!(!CodingAgentKind::Pi.profile().auto_self_clear_supported && requested);
}

#[test]
fn heuristic_agent_metadata_cannot_authorize_pi_mutation() {
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "unrelated-cmd".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Unrelated cmd recipe".to_string(),
            command: "cmd".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    let configured_args = vec!["/C".to_string(), "pi".to_string()];
    let (heuristic_id, _) = resolve_actual_agent("cmd", &configured_args, None, None, &settings);
    assert_eq!(heuristic_id.as_deref(), Some("unrelated-cmd"));

    let mut args = configured_args;
    assert!(!maybe_inject_pi_resume(
        Some(CodingAgentKind::Pi),
        None,
        false,
        "cmd",
        &mut args,
    ));
    assert_eq!(args, vec!["/C", "pi"]);
}

#[test]
fn inject_antigravity_resume_prefixes_direct_agy_args() {
    let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
    assert!(super::inject_antigravity_resume("agy", &mut args));
    assert_eq!(
        args,
        vec![
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );

    let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
    assert!(super::inject_antigravity_resume("antigravity", &mut args));
    assert_eq!(
        args,
        vec![
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_inserts_into_cmd_tokenized_wrapper() {
    let mut args = vec![
        "/C".to_string(),
        "agy".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];
    assert!(super::inject_antigravity_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "agy".to_string(),
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_inserts_into_embedded_cmd_string() {
    let mut args = vec!["/K".to_string(), "git pull && agy -m gpt-5".to_string()];
    assert!(super::inject_antigravity_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/K".to_string(),
            "git pull && agy --continue -m gpt-5".to_string()
        ]
    );
}

#[test]
fn inject_antigravity_resume_skips_when_continue_or_conversation_present() {
    for skip_args in [
        vec!["--continue".to_string(), "gpt-5".to_string()],
        vec!["-c".to_string(), "gpt-5".to_string()],
        vec!["--conversation".to_string(), "abc123".to_string()],
        vec!["--conversation=abc123".to_string()],
        vec!["--CONTINUE".to_string(), "gpt-5".to_string()],
        vec!["--Conversation".to_string(), "abc123".to_string()],
    ] {
        let mut args = skip_args.clone();
        assert!(
            !super::inject_antigravity_resume("agy", &mut args),
            "args={skip_args:?}"
        );
        assert_eq!(args, skip_args);
    }

    // Tokenized cmd form also skips on a resume marker after the executable.
    let mut args = vec![
        "/C".to_string(),
        "agy".to_string(),
        "--conversation".to_string(),
        "abc123".to_string(),
    ];
    assert!(!super::inject_antigravity_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "agy".to_string(),
            "--conversation".to_string(),
            "abc123".to_string()
        ]
    );
}

#[test]
fn inject_codex_resume_prefixes_direct_codex_args() {
    let mut args = vec![
        "-m".to_string(),
        "gpt-5".to_string(),
        "-c".to_string(),
        "model_reasoning_effort=\"high\"".to_string(),
    ];

    assert!(inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=\"high\"".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_inserts_into_cmd_tokenized_wrapper() {
    let mut args = vec![
        "/C".to_string(),
        "codex".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];

    assert!(inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/C".to_string(),
            "codex".to_string(),
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_inserts_into_embedded_cmd_wrapper() {
    let mut args = vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string()];

    assert!(inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec![
            "/K".to_string(),
            "git pull && codex resume --last -m gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_existing_resume_tokens() {
    let mut args = vec![
        "resume".to_string(),
        "--last".to_string(),
        "-m".to_string(),
        "gpt-5".to_string(),
    ];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_explicit_fork_subcommand() {
    let mut args = vec!["fork".to_string(), "--last".to_string()];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(args, vec!["fork".to_string(), "--last".to_string()]);
}

#[test]
fn inject_codex_resume_skips_explicit_exec_subcommand_after_options() {
    let mut args = vec![
        "-m".to_string(),
        "gpt-5".to_string(),
        "exec".to_string(),
        "--json".to_string(),
    ];

    assert!(!inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "exec".to_string(),
            "--json".to_string(),
        ]
    );
}

#[test]
fn inject_codex_resume_skips_explicit_help_subcommand_in_cmd_wrapper() {
    let mut args = vec!["/C".to_string(), "codex".to_string(), "help".to_string()];

    assert!(!inject_codex_resume("cmd.exe", &mut args));
    assert_eq!(
        args,
        vec!["/C".to_string(), "codex".to_string(), "help".to_string()]
    );
}

#[test]
fn inject_codex_resume_ignores_resume_text_inside_config_value() {
    let mut args = vec![
        "-c".to_string(),
        "instruction=\"resume later\"".to_string(),
        "--search".to_string(),
    ];

    assert!(inject_codex_resume("codex", &mut args));
    assert_eq!(
        args,
        vec![
            "resume".to_string(),
            "--last".to_string(),
            "-c".to_string(),
            "instruction=\"resume later\"".to_string(),
            "--search".to_string(),
        ]
    );
}

#[test]
fn resolve_actual_agent_keeps_requested_agent_when_shell_validates_it() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "codex",
        &["-m".to_string(), "gpt-5".to_string()],
        Some("codex"),
        Some("Codex Stable"),
        &settings,
    );

    assert_eq!(
        resolved,
        (Some("codex".to_string()), Some("Codex Stable".to_string()))
    );
}

#[test]
fn resolve_actual_agent_keeps_requested_agent_when_normalized_command_matches() {
    let mut settings = test_settings();
    settings.agents.push(AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "codex-yolo".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Codex Yolo".to_string(),
        command: "codex --yolo".to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    });

    let resolved = resolve_actual_agent(
        "codex",
        &["--yolo".to_string()],
        Some("codex-yolo"),
        Some("Codex Yolo"),
        &settings,
    );

    assert_eq!(
        resolved,
        (
            Some("codex-yolo".to_string()),
            Some("Codex Yolo".to_string())
        )
    );
}

#[test]
fn resolve_actual_agent_falls_back_to_detected_label_when_validated_match_has_no_stored_label() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "codex",
        &["-m".to_string(), "gpt-5".to_string()],
        Some("codex"),
        None,
        &settings,
    );

    assert_eq!(
        resolved,
        (Some("codex".to_string()), Some("Codex".to_string()))
    );
}

#[test]
fn resolve_actual_agent_clears_requested_agent_when_shell_is_unresolved() {
    let settings = test_settings();

    let resolved = resolve_actual_agent(
        "powershell.exe",
        &["-NoLogo".to_string()],
        Some("codex"),
        Some("Codex"),
        &settings,
    );

    assert_eq!(resolved, (None, None));
}

#[test]
fn resolve_actual_agent_uses_shell_resolved_agent_on_mismatch() {
    let settings = test_settings();

    let resolved = resolve_actual_agent("claude", &[], Some("codex"), Some("Codex"), &settings);

    assert_eq!(
        resolved,
        (Some("claude".to_string()), Some("Claude Code".to_string()))
    );
}

#[test]
fn resolve_agent_from_shell_skips_invalid_configured_command() {
    let mut settings = test_settings();
    settings.agents = vec![AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "broken-codex".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Broken Codex".to_string(),
        command: "codex \"unterminated".to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }];

    let resolved = resolve_agent_from_shell("codex", &[], &settings);

    assert_eq!(resolved, (None, None));
}

#[test]
fn effective_restart_skip_auto_resume_defaults_to_true_for_none() {
    // No explicit value → preserve legacy "fresh conversation" semantics
    // used by SessionItem, ProjectPanel context menu, AcDiscoveryPanel.
    assert!(super::effective_restart_skip_auto_resume(None));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_false() {
    // Deferred-wake path (ProjectPanel.handleReplicaClick) MUST be able
    // to opt in to provider auto-resume; otherwise antigravity/codex/claude
    // sessions re-open with a blank slate instead of continuing.
    assert!(!super::effective_restart_skip_auto_resume(Some(false)));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_true() {
    // Explicit true still works (future-proof against a caller that
    // wants to be explicit rather than rely on the default).
    assert!(super::effective_restart_skip_auto_resume(Some(true)));
}

// ── (#630/#631) restart_skip_auto_resume_with_intent: the call site uses this
//    exact composition to decide resume-vs-fresh on a restart. ──

#[test]
fn restart_intent_explicit_restart_button_is_fresh() {
    // "Restart Session" (None) on a session with no stored intent => fresh.
    assert!(super::restart_skip_auto_resume_with_intent(false, None));
}

#[test]
fn restart_intent_normal_member_reopen_resumes() {
    // Branch A reopen of a NORMAL member (no stored intent, Some(false))
    // => resume, unchanged from today.
    assert!(!super::restart_skip_auto_resume_with_intent(
        false,
        Some(false)
    ));
}

#[test]
fn restart_intent_deferred_fresh_member_reopen_stays_fresh() {
    // #631 closure: a "Restart Session"-then-app-restart member defers, then
    // reopens via Branch A (Some(false)), but its persisted fresh intent wins.
    assert!(super::restart_skip_auto_resume_with_intent(
        true,
        Some(false)
    ));
}

#[test]
fn restart_intent_stored_fresh_with_restart_button_is_fresh() {
    // Stored fresh AND an explicit restart => still fresh (no regression).
    assert!(super::restart_skip_auto_resume_with_intent(true, None));
}

// ── (#747) carry_communication_for_restart: shared decision for the
//    Branch-A reopen (restart_session_inner_with_activation) AND the
//    startup wake arm (lib.rs restore loop), which reuses this helper with
//    the persisted intent. This matrix pins the fresh gate for BOTH. ──

#[test]
fn carry_communication_for_restart_matrix() {
    let visible_hand = || {
        Some(crate::session::session::SessionCommunication {
            kind: crate::session::session::SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            message: None,
        })
    };
    let hidden_hand = || {
        Some(crate::session::session::SessionCommunication {
            kind: crate::session::session::SessionCommunicationKind::RaiseHand,
            visible: false,
            updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            message: None,
        })
    };

    // (visible raise, fresh=false): the reopen resumes the conversation,
    // so the pending user-attention marker carries.
    let carried = super::carry_communication_for_restart(visible_hand(), false)
        .expect("visible hand must carry on a resuming reopen");
    assert!(carried.visible);
    assert_eq!(carried.updated_at, "2026-06-30T11:00:00+00:00");

    // (visible raise, fresh=true): a fresh restart abandons the old
    // conversation; the raise belonged to it and drops.
    assert!(super::carry_communication_for_restart(visible_hand(), true).is_none());

    // (hidden raise, fresh=false): non-visible payloads never carry.
    assert!(super::carry_communication_for_restart(hidden_hand(), false).is_none());

    // (None, fresh=false): nothing to carry.
    assert!(super::carry_communication_for_restart(None, false).is_none());
}

// ── #599 R1 effective_create_skip_auto_resume tests ──

#[test]
fn effective_create_skip_auto_resume_defaults_to_true_for_none() {
    // No explicit value: legacy fresh-create default used by create-in-place,
    // new-agent, open-agent, CLI, and web call sites (no --continue).
    assert!(super::effective_create_skip_auto_resume(None));
}

#[test]
fn effective_create_skip_auto_resume_respects_explicit_false() {
    // #599 R1: reopening a coordinator destroyed by auto-close (#552/#580)
    // or manual close (#588) opts in to resume; --continue is then injected
    // (still subject to the claude_project_exists disk gate downstream).
    assert!(!super::effective_create_skip_auto_resume(Some(false)));
}

#[test]
fn effective_create_skip_auto_resume_respects_explicit_true() {
    // Explicit true still skips resume (parity with the restart helper).
    assert!(super::effective_create_skip_auto_resume(Some(true)));
}

// ── #599 R2 claude_projects_dir_for_config_dir tests ──

#[test]
fn claude_projects_dir_for_config_dir_joins_projects_and_mangled() {
    // An explicit CLAUDE_CONFIG_DIR (already AC-expanded) maps to
    // <base>/projects/<mangled-cwd>, mirroring the wrapper resolver output.
    let base = if cfg!(windows) {
        "C:\\Users\\maria\\.claude-env"
    } else {
        "/home/maria/.claude-env"
    };
    let cwd = if cfg!(windows) { "C:\\x" } else { "/home/x" };
    let resolved = super::claude_projects_dir_for_config_dir(base, cwd);
    let expected = std::path::PathBuf::from(base)
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, expected);
}

#[test]
fn claude_projects_dir_for_config_dir_expands_percent_envvar_in_base() {
    // Directly covers the lifted expand_env_var_refs: a %VAR% ref in the
    // config dir resolves against the process env before the projects join.
    let var = "AC_TEST_599_CONFIG_DIR";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    // SAFETY: env state is process-global; unique name avoids cross-test races.
    std::env::set_var(var, custom_base.to_str().unwrap());
    let resolved = super::claude_projects_dir_for_config_dir(&format!("%{}%", var), "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, expected);
}

#[test]
fn local_resume_probe_env_layer_uses_config_dir_override() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    let cwd = "C:\\Users\\Test\\repo";

    let resolved = super::claude_projects_dir_for_config_dir(custom_base.to_str().unwrap(), cwd);

    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, expected);
}

#[test]
fn local_resume_probe_env_layer_expands_powershell_envvar() {
    let var = "AC_TEST_894_CONFIG_DIR";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    std::env::set_var(var, custom_base.to_str().unwrap());

    let resolved =
        super::claude_projects_dir_for_config_dir(&format!("$env:{}\\.claude", var), "C:\\x");

    std::env::remove_var(var);
    let expected = PathBuf::from(format!("{}\\.claude", custom_base.to_string_lossy()))
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, expected);
}

// ── should_inject_continue tests (issue #82, plan §8.1) ──

#[test]
fn should_inject_continue_returns_false_when_not_claude() {
    assert!(!should_inject_continue(false, false, true, "codex"));
}

#[test]
fn should_inject_continue_returns_false_when_skip_overrides_existing_dir() {
    // G4 strengthening: lock the predicate against future refactors that
    // re-order early-return clauses. Explicit fixture, not "all permissive".
    assert!(!should_inject_continue(true, true, true, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_dir_missing() {
    assert!(!should_inject_continue(true, false, false, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_already_present() {
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --continue"
    ));
}

#[test]
fn should_inject_continue_returns_true_for_canonical_resume_case() {
    assert!(should_inject_continue(true, false, true, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_with_value_present() {
    // R2.4 / G2: the GNU long-option-with-value form must also suppress
    // re-injection.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --continue=somevalue"
    ));
}

#[test]
fn should_inject_continue_returns_false_when_uppercase_continue_present() {
    // D4 #6: case-insensitivity regression fence.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --CONTINUE"
    ));
}

#[test]
fn should_inject_continue_returns_false_when_short_form_present() {
    // D4 #7: -c short-form regression fence.
    assert!(!should_inject_continue(true, false, true, "claude -c"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_in_cmd_wrapper() {
    // D4 #8: token-level scan, not arg-index scan.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "cmd /C claude --continue"
    ));
}

#[test]
fn should_inject_continue_returns_true_when_unrelated_continue_substring() {
    // D4 #9: token-equality fence — `--continued-mode` is NOT `--continue`.
    // Guards against a future regression to substring matching.
    assert!(should_inject_continue(
        true,
        false,
        true,
        "claude --continued-mode something"
    ));
}

// ── (#756) mirror_forces_fresh truth table ──

#[test]
fn mirror_forces_fresh_truth_table() {
    // Forces fresh ONLY when: resume requested + coordinator + mirror pending.
    assert!(super::mirror_forces_fresh(false, true, true));
    // Any other combination must not force.
    assert!(!super::mirror_forces_fresh(false, true, false));
    assert!(!super::mirror_forces_fresh(false, false, true));
    assert!(!super::mirror_forces_fresh(false, false, false));
    assert!(!super::mirror_forces_fresh(true, true, true));
    assert!(!super::mirror_forces_fresh(true, true, false));
    assert!(!super::mirror_forces_fresh(true, false, true));
    assert!(!super::mirror_forces_fresh(true, false, false));
}

// ── (#756) should_inject_fresh_session_id decision table ──

#[test]
fn should_inject_fresh_session_id_true_for_claude_skip_clean_argv() {
    assert!(super::should_inject_fresh_session_id(
        true,
        true,
        "claude --dangerously-skip-permissions"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_when_not_skipping_resume() {
    assert!(!super::should_inject_fresh_session_id(
        true,
        false,
        "claude --dangerously-skip-permissions"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_when_not_claude() {
    assert!(!super::should_inject_fresh_session_id(
        false,
        true,
        "codex --dangerously-bypass-approvals-and-sandbox"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_for_each_identity_veto_token() {
    // User-configured identity flags win; the rider must never stack onto
    // them (stacking is a hard CLI error, Q2-verified).
    for cmd in [
        "claude --session-id 7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
        "claude --session-id=7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
        "claude --resume",
        "claude --resume=abc",
        "claude -r",
        "claude --continue",
        "claude --continue=abc",
        "claude -c",
        "claude --fork-session",
        "claude --SESSION-ID x", // case-insensitive token match
        "claude --Continue",
    ] {
        assert!(
            !super::should_inject_fresh_session_id(true, true, cmd),
            "identity flag in {:?} must veto the rider",
            cmd
        );
    }
}

#[test]
fn should_inject_fresh_session_id_true_when_unrelated_flag_substring() {
    // Token-equality fence: `--session-identity` is NOT `--session-id`,
    // `--recover` is NOT `-r`.
    assert!(super::should_inject_fresh_session_id(
        true,
        true,
        "claude --session-identity thing --recover"
    ));
}

// ── (#2454) transcript_id_from_args ──

const TRANSCRIPT_UUID: &str = "7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f";

fn transcript_id_of(args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    super::transcript_id_from_args(&args)
}

// Test 1.
#[test]
fn transcript_id_reads_the_spaced_session_id() {
    assert_eq!(
        transcript_id_of(&["--session-id", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 2.
#[test]
fn transcript_id_reads_the_equals_session_id() {
    let arg = format!("--session-id={TRANSCRIPT_UUID}");
    assert_eq!(transcript_id_of(&[&arg]).as_deref(), Some(TRANSCRIPT_UUID));
}

// Test 3: the `cmd` spawn shape keeps the flag inside the last element; an
// argv scan that does not split on whitespace misses it.
#[test]
fn transcript_id_reads_the_cmd_shape_inside_one_argument() {
    let arg = format!("C:/Tools/claude --session-id {TRANSCRIPT_UUID}");
    assert_eq!(
        transcript_id_of(&["/C", &arg]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 4.
#[test]
fn transcript_id_reads_resume_in_both_forms() {
    let equals = format!("--resume={TRANSCRIPT_UUID}");
    assert_eq!(
        transcript_id_of(&["--resume", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
    assert_eq!(
        transcript_id_of(&[&equals]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 5 (guard): `-r` carries a real UUID on purpose, so treating `-r` as
// `--resume` would return it and turn this red.
#[test]
fn transcript_id_is_none_for_flags_that_name_no_file() {
    assert_eq!(transcript_id_of(&["--continue"]), None);
    assert_eq!(transcript_id_of(&["-c"]), None);
    assert_eq!(transcript_id_of(&["-r", TRANSCRIPT_UUID]), None);
    assert_eq!(transcript_id_of(&[]), None);
}

// Test 6 (guard).
#[test]
fn transcript_id_is_none_for_a_non_uuid_value() {
    assert_eq!(transcript_id_of(&["--session-id", "not-a-uuid"]), None);
    assert_eq!(transcript_id_of(&["--resume=abc"]), None);
}

// Test 7 (guard).
#[test]
fn transcript_id_matches_the_flag_case_insensitively() {
    assert_eq!(
        transcript_id_of(&["--SESSION-ID", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 8 (guard): a trailing flag with no value yields `None`, no panic.
#[test]
fn transcript_id_is_none_for_a_trailing_flag_without_value() {
    assert_eq!(transcript_id_of(&["claude", "--session-id"]), None);
}

// ── Issue #107 Round 5 §R5.8.6 — build_title_prompt_appendage idempotence ──
//
// Tempdir naming starts with `wg-` so `find_workgroup_task_path_for_cwd`'s
// ancestor walk finds the cwd itself as the wg ancestor. The three tests
// pin gates (3), (4), and the happy path. Path-walk gate (1) failure is
// exercised by the existing `find_workgroup_task_path_for_cwd` tests in
// `session/session.rs`. Read-failure gate (2) requires fault-injecting
// `std::fs::read_to_string`, which is not worth the harness for a thin
// orchestrator.

// ── Issue #186 — resolve_claude_projects_dir ──

fn write_wrapper(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn resolve_claude_projects_dir_uses_home_for_bare_claude() {
    // Default install → fall back to <home>/.claude/projects/<mangled>.
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::resolve_claude_projects_dir("claude", &[], cwd);
    // Skip if the test host has no home dir (CI sandboxes sometimes don't).
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_uses_home_for_direct_claude_exe_path() {
    // Direct executable path with file_stem == "claude" → still default base.
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::resolve_claude_projects_dir("C:\\Tools\\claude.exe", &[], cwd);
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_returns_none_when_no_claude_token() {
    let resolved =
        super::resolve_claude_projects_dir("powershell.exe", &["-NoLogo".to_string()], "C:\\x");
    assert!(resolved.is_none());
}

#[test]
fn resolve_claude_projects_dir_parses_wrapper_with_set_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], cwd);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_strips_quotes_around_value() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR=\"{}\"\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_falls_back_when_wrapper_lacks_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = write_wrapper(tmp.path(), "claude-mb.cmd", "@echo off\r\nclaude %*\r\n");
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_falls_back_when_wrapper_missing() {
    let resolved = super::resolve_claude_projects_dir(
        "C:\\definitely\\not\\there\\claude-mb.cmd",
        &[],
        "C:\\x",
    );
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_finds_claude_token_in_cmd_wrapper_args() {
    // shell=cmd.exe, args=["/K", "<abs path to claude-mb.cmd>", "--effort", "max"]
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved = super::resolve_claude_projects_dir(
        "cmd.exe",
        &[
            "/K".to_string(),
            wrapper.to_str().unwrap().to_string(),
            "--effort".to_string(),
            "max".to_string(),
        ],
        "C:\\x",
    );
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string() {
    // Embedded form — per-arg whitespace split must surface claude-mb.cmd.
    // Skip on hosts where the temp dir contains spaces (split would
    // fragment the wrapper path); the cmd-wrapped-args test above
    // already covers spaced paths via direct token form.
    let tmp = tempfile::tempdir().unwrap();
    if tmp.path().to_string_lossy().contains(' ') {
        return;
    }
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let combined = format!("git pull && {} --effort max", wrapper.display());
    let resolved =
        super::resolve_claude_projects_dir("cmd.exe", &["/K".to_string(), combined], "C:\\x");
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_ignores_oversized_wrapper() {
    // Large file (> 64 KiB cap) → treated as not-a-wrapper, fall back.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("claude-mb.cmd");
    let mut body = String::with_capacity(80 * 1024);
    body.push_str("set CLAUDE_CONFIG_DIR=C:\\should-not-be-read\r\n");
    body.push_str(&"x".repeat(80 * 1024));
    std::fs::write(&path, body).unwrap();
    let resolved = super::resolve_claude_projects_dir(path.to_str().unwrap(), &[], "C:\\x");
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_parses_cmd_quoted_whole_assignment() {
    // `set "CLAUDE_CONFIG_DIR=<path with spaces>"` — canonical cmd idiom
    // when the value contains spaces or shell metacharacters.
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset \"CLAUDE_CONFIG_DIR={}\"\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_expands_percent_envvar_value() {
    let var = "AC_TEST_186_BASE_PCT";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    // SAFETY: env state is process-global; unique name avoids cross-test races.
    std::env::set_var(var, custom_base.to_str().unwrap());
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR=%{}%\r\nclaude %*\r\n",
            var
        ),
    );
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_expands_powershell_envvar_value() {
    let var = "AC_TEST_186_BASE_PS";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    std::env::set_var(var, custom_base.to_str().unwrap());
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.ps1",
        &format!(
            "$env:CLAUDE_CONFIG_DIR = \"$env:{}\"\r\nclaude @args\r\n",
            var
        ),
    );
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_preserves_unknown_envvar_literal() {
    // Unknown var → literal preserved → resulting path is_dir() will be
    // false at the call site, but parse must succeed (return Some).
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            "@echo off\r\nset CLAUDE_CONFIG_DIR=%AC_TEST_186_DEFINITELY_UNSET%\\\\.claude-mb\r\nclaude %*\r\n",
        );
    let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let resolved = resolved.expect("parser must return Some even when var is unset");
    let s = resolved.to_string_lossy();
    assert!(
        s.contains("%AC_TEST_186_DEFINITELY_UNSET%"),
        "expected literal preservation, got {s}"
    );
}

#[test]
fn resolve_claude_projects_dir_parses_wrapper_with_export_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.sh",
        &format!(
            "#!/usr/bin/env bash\nexport CLAUDE_CONFIG_DIR={}\nexec claude \"$@\"\n",
            custom_base.display()
        ),
    );
    let resolved =
        super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "/home/test/repo");
    let expected =
        custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(
                "/home/test/repo",
            ));
    assert_eq!(resolved, Some(expected));
}

#[cfg(unix)]
fn agent_path_2589_wrapper_fixture(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("user-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        &bin,
        name,
        &format!(
            "#!/usr/bin/env bash
export CLAUDE_CONFIG_DIR={}
exec claude \"$@\"
",
            custom_base.display()
        ),
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    (tmp, bin, custom_base)
}

#[cfg(unix)]
fn agent_path_2589_search_path(extra: Option<&std::path::Path>) -> std::ffi::OsString {
    crate::config::agent_path::compose(&crate::config::agent_path::SearchPathInputs {
        inherited: Some(std::ffi::OsString::from("/bin:/usr/bin")),
        home: None,
        login_shell_path: extra.map(|dir| dir.as_os_str().to_os_string()),
    })
}

#[cfg(unix)]
fn agent_path_2589_default_base(cwd: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd))
    })
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_in_a_user_bin_dir_is_parsed() {
    let (_tmp, bin, custom_base) = agent_path_2589_wrapper_fixture("claude-mb.sh");
    let cwd = "/home/test/repo";
    let resolved = super::resolve_claude_projects_dir_in(
        "claude-mb.sh",
        &[],
        cwd,
        &agent_path_2589_search_path(Some(&bin)),
    );
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_without_the_added_dir_falls_back() {
    let (_tmp, _bin, custom_base) = agent_path_2589_wrapper_fixture("claude-mb.sh");
    let cwd = "/home/test/repo";
    let resolved = super::resolve_claude_projects_dir_in(
        "claude-mb.sh",
        &[],
        cwd,
        &agent_path_2589_search_path(None),
    );
    assert_eq!(resolved, agent_path_2589_default_base(cwd));
    assert_ne!(
        resolved,
        Some(
            custom_base
                .join("projects")
                .join(crate::session::session::mangle_cwd_for_claude(cwd))
        )
    );
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_without_extension_uses_the_default_base() {
    let (_tmp, bin, _custom_base) = agent_path_2589_wrapper_fixture("claude-mb");
    let search_path = agent_path_2589_search_path(Some(&bin));
    assert!(
        crate::config::agent_command::resolve_program_in("claude-mb", &search_path).is_some(),
        "the extensionless wrapper now resolves through the added dir"
    );
    let cwd = "/home/test/repo";
    let resolved = super::resolve_claude_projects_dir_in("claude-mb", &[], cwd, &search_path);
    assert_eq!(resolved, agent_path_2589_default_base(cwd));
}

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
    let err = super::create_session_inner_impl(
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
    let outcome = super::create_session_inner_impl(
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

    let err = super::restart_session_inner_with_intent(
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

    // Root-agent cwd: allowed even when the dir is missing.
    let app = session_test_app(
        AppSettings::default(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let root = crate::config::root_agent::root_agent_dir().expect("root dir resolves");
    assert!(
        enforce_creation_gate(app.handle(), &root, CreationGateEnforcement::Enforce)
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
    assert!(
        enforce_creation_gate(
            app2.handle(),
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
    let err = enforce_creation_gate(app3.handle(), &outside, CreationGateEnforcement::Enforce)
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
    let resolved = super::resolve_create_session_cwd(None);
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
    super::create_session_inner(
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

// --- #1271 configured default-shell propagation --------------------------

/// Captures every `BackendSpawnSpec` the PTY manager hands to the backend,
/// so tests can assert the resolved-agent host-shell snapshot propagated
/// unchanged from the creation seam. Spawned ids count as live sessions so
/// the finalized-create path can display them, mirroring
/// `ScriptedSpawnBackend`.
#[derive(Default)]
struct CapturingSpawnBackend {
    specs: Mutex<Vec<crate::pty::backend::BackendSpawnSpec>>,
    live: Mutex<HashSet<Uuid>>,
    /// #1873 - every id the manager asked this backend to tear down.
    killed: Mutex<Vec<Uuid>>,
}

impl PtyBackend for CapturingSpawnBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.live.lock().unwrap().insert(spec.id);
            self.specs.lock().unwrap().push(spec);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Err(crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        self.live.lock().unwrap().remove(&id);
        self.killed.lock().unwrap().push(id);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
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

// --- #1873 Muse workspace-latest automatic resume --------------------------

/// Test-only Muse settings row appended to `test_settings()`. Production ships
/// no Muse row here; the #1860 catalog is the only built-in.
fn muse_test_settings(command: &str) -> AppSettings {
    let mut settings = test_settings();
    settings.agents.push(AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "muse".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Muse Code".to_string(),
        command: command.to_string(),
        color: "#0668E1".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    });
    settings
}

fn muse_spawn_for(
    settings: &AppSettings,
    cwd: &str,
) -> crate::config::agent_command::AgentSpawnCommand {
    super::build_configured_agent_spawn_for_cwd(settings, "muse", cwd, None)
        .expect("resolve the configured Muse spawn")
        .expect("muse is configured in muse_test_settings")
}

const MUSE_SUPPORTED_HOST: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// The plan's LITERAL workspace-latest selector. Deliberately NOT derived from
/// `MUSE_PROFILE.resume_tokens`: these tests must fail when the constant under
/// test carries a wrong token (Grinch mutation `--last` -> `--first`).
fn muse_resume_tokens() -> Vec<String> {
    vec!["resume".to_string(), "--last".to_string()]
}

/// Effective argv the backend must see for a resume-intent Muse launch on this host.
fn expected_muse_resume_argv() -> Vec<String> {
    if MUSE_SUPPORTED_HOST {
        muse_resume_tokens()
    } else {
        Vec::new()
    }
}

/// #1873 - the predicate is true only for the exact trusted recipe; every veto
/// is independent, and the injector fails closed without mutation on a wrong
/// kind, fresh intent, missing provenance, or a second call.
#[test]
fn muse_resume_eligibility_is_exact_and_fail_closed() {
    let resolve = |settings: &AppSettings| {
        crate::config::agent_command::resolve_agent_spawn_command(
            settings, "muse", None, None, false,
        )
        .expect("Muse test spawn should resolve without filesystem preparation")
    };
    let bare = resolve(&muse_test_settings("muse"));
    let absolute = resolve(&muse_test_settings("/opt/muse/bin/muse"));
    let no_args: Vec<String> = Vec::new();

    // Positives (host-gated: Windows compiles the same predicate to false).
    assert_eq!(
        super::trusted_muse_auto_resume_spawn(Some(&bare), "muse", &no_args),
        MUSE_SUPPORTED_HOST
    );
    assert_eq!(
        super::trusted_muse_auto_resume_spawn(Some(&absolute), "/opt/muse/bin/muse", &no_args),
        MUSE_SUPPORTED_HOST
    );

    // Vetoes: ad-hoc (no provenance), shell mismatch, args mismatch.
    assert!(!super::trusted_muse_auto_resume_spawn(
        None, "muse", &no_args
    ));
    assert!(!super::trusted_muse_auto_resume_spawn(
        Some(&bare),
        "/usr/bin/muse",
        &no_args
    ));
    assert!(!super::trusted_muse_auto_resume_spawn(
        Some(&bare),
        "muse",
        &["--workspace".to_string(), "/srv/work".to_string()]
    ));

    // Any configured argument suppresses injection, even when the recipe matches.
    for command in [
        "muse resume --last",
        "muse resume 0f3d2a5c-9c1e-4b7e-8f6a-2b1c3d4e5f60",
        "muse --workspace /srv/work",
        "muse --root /srv/root",
        "muse --no-session-log",
        "muse --help",
        "muse --version",
        "muse \"summarize the open pull requests\"",
        "muse --future-flag",
    ] {
        let spawn = resolve(&muse_test_settings(command));
        assert!(
            !super::trusted_muse_auto_resume_spawn(Some(&spawn), &spawn.shell, &spawn.shell_args),
            "command={command:?}"
        );
    }

    // Prefix names, mixed case and wrappers are never Muse identity; `muse.exe`
    // IS Muse kind (exact stem) but its file name is not `muse`, so only the
    // injection predicate rejects it.
    for command in [
        "muse-agent",
        "Muse",
        "muse.exe",
        "/opt/muse/bin/",
        "env muse",
        "cmd /C muse",
        "bash -lc muse",
    ] {
        let spawn = resolve(&muse_test_settings(command));
        assert!(
            !super::trusted_muse_auto_resume_spawn(Some(&spawn), &spawn.shell, &spawn.shell_args),
            "command={command:?}"
        );
    }

    // Containers never qualify.
    let mut container = muse_test_settings("muse");
    container.agents.last_mut().unwrap().backend = crate::config::settings::AgentBackendConfig {
        kind: SessionBackendKind::ContainerTransport,
        image: Some("ghcr.io/example/muse:beta".to_string()),
    };
    let container_spawn = resolve(&container);
    assert_eq!(
        SessionBackendKind::from(&container_spawn.backend),
        SessionBackendKind::ContainerTransport
    );
    assert!(!super::trusted_muse_auto_resume_spawn(
        Some(&container_spawn),
        "muse",
        &no_args
    ));

    // Injector: resume intent + trusted recipe mutates exactly once.
    let mut args: Vec<String> = Vec::new();
    assert_eq!(
        super::maybe_inject_muse_resume(
            Some(CodingAgentKind::Muse),
            Some(&bare),
            false,
            "muse",
            &mut args
        ),
        MUSE_SUPPORTED_HOST
    );
    assert_eq!(args, expected_muse_resume_argv());
    // A second call finds a non-empty vector and fails closed without mutation.
    let before = args.clone();
    assert!(!super::maybe_inject_muse_resume(
        Some(CodingAgentKind::Muse),
        Some(&bare),
        false,
        "muse",
        &mut args
    ));
    assert_eq!(args, before);

    // Fresh intent, wrong/no kind, and missing provenance never mutate.
    for (kind, spawn, fresh) in [
        (Some(CodingAgentKind::Muse), Some(&bare), true),
        (Some(CodingAgentKind::Codex), Some(&bare), false),
        (Some(CodingAgentKind::Pi), Some(&bare), false),
        (None, Some(&bare), false),
        (Some(CodingAgentKind::Muse), None, false),
    ] {
        let mut args: Vec<String> = Vec::new();
        assert!(!super::maybe_inject_muse_resume(
            kind, spawn, fresh, "muse", &mut args
        ));
        assert!(args.is_empty());
    }
    // Configured arguments: the injector leaves them byte-identical.
    let manual = resolve(&muse_test_settings(
        "muse --workspace /tmp/codex resume --last",
    ));
    let mut manual_args = manual.shell_args.clone();
    assert!(!super::maybe_inject_muse_resume(
        Some(CodingAgentKind::Muse),
        Some(&manual),
        false,
        &manual.shell,
        &mut manual_args
    ));
    assert_eq!(manual_args, manual.shell_args);
}

/// #1873 - fresh (`true`), resume (`false`) and durable-fresh restore intent all
/// flow through the single launch seam; only resume intent yields `resume --last`,
/// and only in the effective vector.
#[tokio::test]
async fn muse_fresh_and_resume_intents_share_one_launch_seam() {
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    let launch = |cwd: String, skip_auto_resume: bool, name: &str| {
        let spawn = muse_spawn_for(&settings, &cwd);
        let name = name.to_string();
        let app = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        async move {
            super::create_session_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                cwd,
                Some(name),
                Some(spawn.trusted_agent_id.clone()),
                Some(spawn.trusted_agent_label.clone()),
                false,
                Vec::new(),
                skip_auto_resume,
                Some(spawn),
                None,
                None,
                CreateSelectionIntent::User,
            )
            .await
            .expect("Muse launch succeeds")
        }
    };

    // Ordinary create / explicit Restart Session / root fresh / mailbox cold /
    // loop cold: skip_auto_resume == true -> configured plain muse.
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = launch(
        fresh_dir.path().to_string_lossy().to_string(),
        true,
        "muse fresh",
    )
    .await;
    assert_eq!(fresh.agent_kind, Some(CodingAgentKind::Muse));
    assert!(fresh.shell_args.is_empty());
    assert_eq!(fresh.effective_shell_args.as_deref(), Some(&[][..]));
    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("fresh launch reached the backend");
        assert_eq!(spec.cmd, "muse");
        assert!(
            spec.args.is_empty(),
            "fresh Muse must stay plain: {:?}",
            spec.args
        );
    }

    // Startup restore / closed-coordinator reopen / dormant root / mailbox and
    // loop known-state wake: skip_auto_resume == false -> muse resume --last.
    let resume_dir = tempfile::tempdir().unwrap();
    let resumed = launch(
        resume_dir.path().to_string_lossy().to_string(),
        false,
        "muse resume",
    )
    .await;
    assert_eq!(resumed.agent_kind, Some(CodingAgentKind::Muse));
    assert!(
        resumed.shell_args.is_empty(),
        "configured argv never receives AC tokens"
    );
    assert_eq!(
        resumed.effective_shell_args.clone().unwrap_or_default(),
        expected_muse_resume_argv()
    );
    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("resume launch reached the backend");
        assert_eq!(spec.cmd, "muse");
        assert_eq!(spec.args, expected_muse_resume_argv());
    }

    // Durable fresh intent on a restore: `start_fresh_on_restore == true` maps to
    // `skip_auto_resume_for_restore(true) == true` -> plain muse, stamped fresh.
    let durable_dir = tempfile::tempdir().unwrap();
    let durable_cwd = durable_dir.path().to_string_lossy().to_string();
    let durable_spawn = muse_spawn_for(&settings, &durable_cwd);
    let restore = crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let durable = super::create_session_inner_for_restore(
        &restore,
        &session_mgr,
        &pty_mgr,
        durable_spawn.shell.clone(),
        durable_spawn.shell_args.clone(),
        durable_cwd,
        Some("muse durable fresh".to_string()),
        Some("muse".to_string()),
        Some(durable_spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        crate::skip_auto_resume_for_restore(true),
        Some(durable_spawn),
        None,
        None,
        Some(true),
        None,
        None,
        None,
    )
    .await
    .expect("durable-fresh restore launches");
    assert!(durable.start_fresh_on_restore);
    assert!(durable.shell_args.is_empty());
    assert_eq!(durable.effective_shell_args.as_deref(), Some(&[][..]));
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 3, "exactly one spawn per launch");
        let spec = specs.last().unwrap();
        assert_eq!(spec.cmd, "muse");
        assert!(
            spec.args.is_empty(),
            "durable fresh must stay plain: {:?}",
            spec.args
        );
    }
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 3);
    close_test_coordinator(&app).await;
}

/// #1873 - a directly configured Muse recipe with a Codex-looking value and
/// manual `resume --last` passes pre-spawn validation, is preserved byte-for-byte,
/// and suppresses injection all the way to the backend.
#[tokio::test]
async fn muse_manual_collision_args_pass_validation_and_reach_backend_unchanged() {
    let settings = muse_test_settings("muse --workspace /tmp/codex resume --last");
    crate::config::settings::validate_agent_commands(&settings)
        .expect("direct Muse arguments are user-authored and valid");
    let configured = vec![
        "--workspace".to_string(),
        "/tmp/codex".to_string(),
        "resume".to_string(),
        "--last".to_string(),
    ];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, "muse");
    assert_eq!(spawn.shell_args, configured);
    assert_eq!(
        CodingAgentKind::detect(&spawn.shell, &spawn.shell_args),
        Some(CodingAgentKind::Muse),
        "direct Muse outranks the Codex-looking value"
    );

    // Resume intent: the configured arguments still win (no injection).
    let created = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse manual".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("manual Muse launch succeeds");
    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(created.shell_args, configured);
    assert_eq!(created.effective_shell_args.as_ref(), Some(&configured));
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].cmd, "muse");
        assert_eq!(
            specs[0].args, configured,
            "argv reaches the backend unchanged"
        );
    }
    close_test_coordinator(&app).await;
}

/// #1873 - the backend positive control: on macOS/Linux the spec and effective
/// vectors carry exactly the profile tokens while the configured argv stays
/// empty; generic agent/profile env flows unchanged; the kind/wire value is
/// Muse/"muse". Unsupported hosts stay plain.
#[tokio::test]
async fn trusted_muse_resume_reaches_backend_effective_argv_only() {
    let mut settings = muse_test_settings("muse");
    settings
        .agents
        .last_mut()
        .unwrap()
        .envs
        .push(crate::config::settings::CodingAgentEnv {
            key: "MUSE_TEST_AGENT_ENV".to_string(),
            value: "agent-row".to_string(),
            source: Default::default(),
            enabled: true,
        });
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("muse".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            crate::config::settings::ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([(
                    "MUSE_TEST_PROFILE_ENV".to_string(),
                    "profile-cell".to_string(),
                )]),
                notes: String::new(),
            },
        );
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("muse".to_string(), "A".to_string());

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, "muse");
    assert!(spawn.shell_args.is_empty());
    let agent_row = ("MUSE_TEST_AGENT_ENV".to_string(), "agent-row".to_string());
    let profile_row = (
        "MUSE_TEST_PROFILE_ENV".to_string(),
        "profile-cell".to_string(),
    );
    assert!(
        spawn.child_env.contains(&agent_row),
        "{:?}",
        spawn.child_env
    );
    assert!(
        spawn.child_env.contains(&profile_row),
        "{:?}",
        spawn.child_env
    );

    let created = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse positive control".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("trusted Muse resume launch succeeds");

    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(created.shell, "muse");
    assert!(
        created.shell_args.is_empty(),
        "configured args stay empty: {:?}",
        created.shell_args
    );
    let effective = created.effective_shell_args.clone().unwrap_or_default();
    let wire = serde_json::to_value(&created).expect("SessionInfo serializes");
    assert_eq!(wire["agentKind"], serde_json::json!("muse"));
    assert_eq!(wire["shellArgs"], serde_json::json!([]));
    // PRIMARY: the literal plan tokens, never read back from the profile constant.
    if MUSE_SUPPORTED_HOST {
        assert_eq!(effective, ["resume", "--last"]);
        assert_eq!(
            wire["effectiveShellArgs"],
            serde_json::json!(["resume", "--last"])
        );
    } else {
        assert!(
            effective.is_empty(),
            "unsupported hosts stay plain: {effective:?}"
        );
        assert_eq!(wire["effectiveShellArgs"], serde_json::json!([]));
    }
    // Secondary cross-check: the profile constant equals the same literal.
    assert_eq!(
        CodingAgentKind::Muse.profile().resume_tokens,
        ["resume", "--last"]
    );
    assert_eq!(effective, expected_muse_resume_argv());

    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1, "exactly one process attempt");
        let spec = &specs[0];
        assert_eq!(spec.cmd, "muse");
        if MUSE_SUPPORTED_HOST {
            assert_eq!(spec.args, ["resume", "--last"]);
        } else {
            assert!(spec.args.is_empty(), "{:?}", spec.args);
        }
        assert_eq!(spec.args, expected_muse_resume_argv());
        assert_eq!(spec.coding_agent, Some(CodingAgentKind::Muse));
        assert_eq!(spec.agent_id.as_deref(), Some("muse"));
        assert!(
            spec.configured_env.contains(&agent_row),
            "agent env reaches the child unchanged: {:?}",
            spec.configured_env
        );
        assert!(
            spec.configured_env.contains(&profile_row),
            "profile env reaches the child unchanged: {:?}",
            spec.configured_env
        );
        assert!(
            !spec
                .configured_env
                .iter()
                .any(|(key, _)| key.to_ascii_uppercase().contains("MUSE_HOME")),
            "no Muse-specific env key is minted: {:?}",
            spec.configured_env
        );
    }
    close_test_coordinator(&app).await;
}

/// #1873 - Root agent-picker selection on a LIVE Muse record omits the restart
/// flag; `super::effective_restart_skip_auto_resume(None)` is fresh, and
/// `execute_restart_transaction` tears the old runtime down once and spawns
/// exactly one replacement with configured plain muse.
#[tokio::test]
async fn muse_live_root_selection_is_one_fresh_replacement() {
    use tauri::Manager;

    assert!(super::effective_restart_skip_auto_resume(None));
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);

    // The live record was a resume launch, so the replacement's plain argv is
    // observable as the ABSENCE of tokens the first spec carried.
    let old = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse live root".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("live Muse launch");
    let old_id = Uuid::parse_str(&old.id).unwrap();
    assert!(backend.has_session(old_id), "the record is live");
    assert_eq!(
        backend.specs.lock().unwrap()[0].args,
        expected_muse_resume_argv()
    );

    let settings_state = app.state::<crate::config::settings::SettingsState>();
    let replacement = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings_state.inner(),
        old_id,
        None,
        None,
        None, // restart flag omitted: backend defaults fresh
        true,
    )
    .await
    .expect("one fresh replacement");
    let new_id = Uuid::parse_str(&replacement.id).unwrap();
    assert_ne!(new_id, old_id, "replacement is a new id");
    assert_eq!(replacement.agent_kind, Some(CodingAgentKind::Muse));
    assert!(replacement.shell_args.is_empty());
    assert_eq!(replacement.effective_shell_args.as_deref(), Some(&[][..]));

    assert_eq!(
        backend.killed.lock().unwrap().clone(),
        vec![old_id],
        "one teardown"
    );
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 2, "exactly one replacement spawn");
        assert_eq!(specs[1].id, new_id);
        assert_eq!(specs[1].cmd, "muse");
        assert!(
            specs[1].args.is_empty(),
            "replacement must be configured plain muse: {:?}",
            specs[1].args
        );
        assert!(!specs[1].args.iter().any(|a| a == "resume" || a == "--last"));
    }
    assert!(!backend.has_session(old_id));
    assert!(backend.has_session(new_id));
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, replacement.id, "old row removed");
    close_test_coordinator(&app).await;
}

#[tokio::test]
#[ignore = "#2854 WIP/technical debt: simulator-dependent C19 reclaim test"]
async fn retained_exited_respawn_c19_completed_restarts_reclaim_more_than_64_entries() {
    let settings = muse_test_settings("muse");
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings.clone(), manager.clone(), pty.clone());
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().into_owned();
    let spawn = muse_spawn_for(&settings, &cwd);
    let initial = super::create_session_inner(
        app.handle(),
        &manager,
        &pty,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("C19 actual successful entries".into()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap();
    let mut current = Uuid::parse_str(&initial.id).unwrap();
    let settings = app.state::<crate::config::settings::SettingsState>();
    for _ in 0..66 {
        let previous = current;
        let result = super::restart_session_inner_with_activation(
            app.handle(),
            &manager,
            &pty,
            settings.inner(),
            previous,
            None,
            None,
            Some(true),
            true,
        )
        .await
        .expect("real successful entry releases capacity before next admission");
        current = Uuid::parse_str(&result.id).unwrap();
        assert_ne!(current, previous);
        assert!(!backend.has_session(previous));
        assert!(backend.has_session(current));
        assert!(manager.read().await.get_session(previous).await.is_none());
    }
    assert_eq!(backend.specs.lock().unwrap().len(), 67);
    assert_eq!(backend.killed.lock().unwrap().len(), 66);
    assert_eq!(manager.read().await.list_sessions().await.len(), 1);
    let report = app
        .state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
    assert!(
        report.persistence_safe,
        "completed entries must be terminal: {:?}",
        report.retained
    );
}

/// #1873 - a spawn-time `Err` makes exactly one resume attempt, surfaces that one
/// diagnostic, publishes no created event, leaves no row, and never retries plain.
#[tokio::test]
async fn muse_spawn_error_is_not_retried_fresh() {
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(FailingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let events = capture_session_lifecycle(&app);
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);

    let err = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse spawn error".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("the resume attempt fails at spawn");
    assert!(err.contains("synthetic spawn failure"), "{err}");

    // Exactly one attempt, rolled back exactly once, no row, no retry.
    assert_eq!(backend.spawned().len(), 1, "one resume attempt");
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend.killed().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rollback removes the pending row exactly once");
    assert_eq!(backend.killed(), backend.spawned());
    assert!(session_mgr.read().await.list_sessions().await.is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        backend.spawned().len(),
        1,
        "no automatic plain-Muse retry follows a failed resume"
    );
    let mut created_events = 0;
    while let Ok((name, _payload)) = events.try_recv() {
        assert_ne!(
            name, "session_created",
            "a failed spawn publishes no created event"
        );
        created_events += usize::from(name == "session_created");
    }
    assert_eq!(created_events, 0);
    close_test_coordinator(&app).await;
}

/// #1873 - through the REAL local backend: a trusted resume launch of a `muse`
/// executable that journals argv and exits 41 succeeds at create (one retained
/// row, one created event, wire "muse", argv `[muse-path, "resume", "--last"]`),
/// then the monitor reports one child-initiated exit with duplicate suppression;
/// no second event, no plain fallback. The retained row still permits a
/// user-selected fresh Restart Session.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn muse_post_spawn_nonzero_exit_is_reported_once_without_fresh_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use tauri::Manager;

    let bin = tempfile::tempdir().unwrap();
    let journal = bin.path().join("argv.journal");
    let muse_path = bin.path().join("muse");
    std::fs::write(
        &muse_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 41\n",
            journal.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&muse_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let muse_path_str = muse_path.to_string_lossy().to_string();
    assert_eq!(
        CodingAgentKind::detect(&muse_path_str, &[]),
        Some(CodingAgentKind::Muse)
    );

    let settings = muse_test_settings(&muse_path_str);
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // The real local backend: the child monitor and exit attribution live in it.
    let git_app = Box::leak(Box::new(
        crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build git watcher app"),
    ));
    let git_watcher = crate::pty::git_watcher::GitWatcher::new(
        Arc::clone(&session_mgr),
        git_app.handle().clone(),
    );
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let output_senders: crate::telegram::manager::OutputSenderMap =
        Arc::new(Mutex::new(HashMap::new()));
    let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::new(
        output_senders,
        idle_detector,
        git_watcher,
        None,
    ));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let events = capture_session_lifecycle(&app);
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, muse_path_str);
    assert!(spawn.shell_args.is_empty());

    let created = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse exit 41".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("spawn succeeds; the nonzero exit is post-spawn");
    let id = Uuid::parse_str(&created.id).unwrap();
    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(
        serde_json::to_value(&created).unwrap()["agentKind"],
        serde_json::json!("muse")
    );
    assert!(created.shell_args.is_empty());
    assert_eq!(
        created.effective_shell_args.clone().unwrap_or_default(),
        muse_resume_tokens()
    );

    // Exactly one created event, for this row.
    let (name, payload) = events
        .recv_timeout(Duration::from_secs(5))
        .expect("the create publishes its one created event");
    assert_eq!(name, "session_created");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["id"], serde_json::json!(created.id));
    assert_eq!(payload["agentKind"], serde_json::json!("muse"));

    // The monitor observes the bounded child exit and reports it once.
    let record = crate::pty::spawn_diagnostics::record_for(id)
        .expect("launch provenance is recorded for the spawned child");
    assert_eq!(
        record.argv(),
        [
            muse_path_str.clone(),
            "resume".to_string(),
            "--last".to_string()
        ]
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let liveness = loop {
        if let Some(liveness) = record.final_liveness() {
            if record.exit_reported() {
                break liveness;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child monitor must report the exit within the bound"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(
        matches!(
            liveness,
            crate::pty::spawn_diagnostics::ChildLiveness::Exited {
                code: 41,
                success: false
            }
        ),
        "bounded nonzero exit must be observed as Exited(41,false), got {liveness:?}"
    );
    assert_eq!(
        record.exit_cause(),
        Some(crate::pty::spawn_diagnostics::ExitCause::ChildInitiated)
    );
    assert!(
        !record.log_child_exit(
            crate::pty::spawn_diagnostics::ExitCause::ChildInitiated,
            &liveness,
            "duplicate-suppression-probe"
        ),
        "a second report of the same exit is suppressed"
    );

    // One journal entry: the resume argv. No plain fallback ran.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let journaled = std::fs::read_to_string(&journal).unwrap();
    assert_eq!(journaled.lines().collect::<Vec<_>>(), vec!["resume --last"]);

    // No second created event and no destroy: the row is retained.
    while let Ok((name, _)) = events.try_recv() {
        assert!(
            name != "session_created" && name != "session_destroyed",
            "unexpected lifecycle event after the child exit: {name}"
        );
    }
    let rows = session_mgr.read().await.list_sessions().await;
    assert_eq!(rows.len(), 1, "the retained row survives the nonzero exit");
    assert_eq!(rows[0].id, created.id);
    assert!(rows[0].shell_args.is_empty(), "configured argv unchanged");

    // The retained row permits a deliberate user-selected fresh Restart Session:
    // one more process attempt, with configured plain argv (empty journal line).
    let settings_state = app.state::<crate::config::settings::SettingsState>();
    let restarted = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings_state.inner(),
        id,
        None,
        None,
        None,
        true,
    )
    .await
    .expect("user-selected fresh Restart Session is available");
    assert_ne!(restarted.id, created.id);
    assert_eq!(restarted.effective_shell_args.as_deref(), Some(&[][..]));
    let restart_deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let journaled = std::fs::read_to_string(&journal).unwrap();
        let lines: Vec<&str> = journaled.lines().collect();
        if lines.len() == 2 {
            assert_eq!(lines, vec!["resume --last", ""]);
            break;
        }
        assert!(
            std::time::Instant::now() < restart_deadline,
            "the fresh restart must journal exactly one plain attempt: {lines:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn configured_default_shell_snapshot_reaches_backend_for_resolved_agent() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    settings.default_shell =
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoProfile".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    // Drive the resolved-agent path exactly as `create_session` does: the
    // resolved agent stays the command-to-run, and the configured host shell
    // (copied from the same config snapshot) travels separately.
    let spawn = super::build_configured_agent_spawn_for_cwd(&test_settings(), "claude", &cwd, None)
        .expect("resolve claude")
        .expect("claude is configured in test_settings");
    let host_shell = crate::pty::backend::ResolvedAgentHostShell {
        program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
        args: vec!["-NoProfile".to_string()],
    };
    let created = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.clone(),
        Some("1271 propagation".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        Some(host_shell),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("resolved-agent create succeeds");
    assert_eq!(created.agent_id.as_deref(), Some("claude"));

    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs
            .last()
            .expect("the resolved-agent create reached the backend");
        assert_eq!(spec.cmd, "claude", "agent program stays the command-to-run");
        let host = spec
            .resolved_agent_host_shell
            .as_ref()
            .expect("host shell snapshot must reach the backend");
        assert_eq!(
            host.program,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert_eq!(host.args, vec!["-NoProfile".to_string()]);
    }
    close_test_coordinator(&app).await;
}

/// #1271 - the observable session metadata seam (`set_pending_effective_shell_args`,
/// surfaced as `effective_shell_args`) keeps the LOGICAL agent argv; the
/// configured host-shell arguments travel only in the backend spec's
/// `resolved_agent_host_shell`. The two must remain distinct: host-shell args
/// are never written into the metadata channel.
#[tokio::test]
async fn configured_default_shell_metadata_keeps_logical_agent_argv_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    settings.default_shell =
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
    settings.default_shell_args = vec!["-NoProfile".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let spawn = super::build_configured_agent_spawn_for_cwd(&test_settings(), "claude", &cwd, None)
        .expect("resolve claude")
        .expect("claude is configured in test_settings");
    let host_shell = crate::pty::backend::ResolvedAgentHostShell {
        program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
        args: vec!["-NoProfile".to_string()],
    };
    let created = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd.clone(),
        Some("1271 metadata".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        true,
        Some(spawn),
        Some(host_shell),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("resolved-agent create succeeds");

    // The claude test agent gets auto-injected logical args (`--session-id`):
    // metadata must carry exactly that logical argv, never the configured
    // host-shell args.
    let effective = created
        .effective_shell_args
        .as_deref()
        .expect("effective args are captured before spawn");
    assert!(
        effective.iter().any(|arg| arg.starts_with("--session-id")),
        "session metadata carries the logical agent argv: {effective:?}"
    );
    assert!(
        !effective.iter().any(|arg| arg == "-NoProfile"),
        "host-shell args must never leak into session metadata: {effective:?}"
    );
    let session = session_mgr
        .read()
        .await
        .get_session(Uuid::parse_str(&created.id).unwrap())
        .await
        .expect("session exists");
    let stored = session
        .effective_shell_args
        .as_deref()
        .expect("stored effective args");
    assert!(
        !stored.iter().any(|arg| arg == "-NoProfile"),
        "the pending-effective metadata channel stays logical: {stored:?}"
    );

    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("create reached the backend");
        assert_eq!(
            spec.args, effective,
            "backend args stay the logical agent argv"
        );
        assert_eq!(
            spec.resolved_agent_host_shell
                .as_ref()
                .expect("host shell present")
                .args,
            vec!["-NoProfile".to_string()],
            "host-shell args travel only in the paired snapshot"
        );
    }
    close_test_coordinator(&app).await;
}

/// #1271 - native twin of the web invalid-input postcondition test: the
/// adapter rejection happens at the TOP of the REAL `spawn_sync`, before
/// any spawn accounting or PTY acquisition, so a rejected configured host
/// leaves no session, no pending/metadata record, no spawn record, no PTY
/// map entry, and no output task. Windows-only (the adapter is the Windows
/// host-shell branch); the backend seam proves the same postcondition with
/// a known id in `adapter_spawn_sync_tests`.
#[cfg(windows)]
#[tokio::test]
async fn configured_default_shell_invalid_input_leaves_no_session_state_native() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let mut settings = test_settings();
    // A conflicting/terminal configured option must fail before any PTY.
    settings.default_shell = "powershell.exe".to_string();
    settings.default_shell_args = vec!["-Command".to_string()];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // The real local backend: the invalid-input rejection lives inside its
    // spawn_sync, so a fake capturing backend would accept the spawn.
    let git_app = Box::leak(Box::new(
        crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build native invalid-input git watcher app"),
    ));
    let git_watcher = crate::pty::git_watcher::GitWatcher::new(
        Arc::clone(&session_mgr),
        git_app.handle().clone(),
    );
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let output_senders: crate::telegram::manager::OutputSenderMap =
        Arc::new(Mutex::new(HashMap::new()));
    let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::new(
        output_senders,
        idle_detector,
        git_watcher,
        None,
    ));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend,
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let error = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "claude".to_string(),
        Vec::new(),
        cwd.clone(),
        Some("1271 native invalid input".to_string()),
        None,
        None,
        false,
        Vec::new(),
        true,
        None,
        Some(crate::pty::backend::ResolvedAgentHostShell {
            program: "powershell.exe".to_string(),
            args: vec!["-Command".to_string()],
        }),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect_err("invalid configured host must fail the create");
    assert!(error.contains("conflicting/terminal"), "{error}");
    assert!(
        error.contains("agent adapter owns command execution"),
        "{error}"
    );

    assert!(
        session_mgr.read().await.list_sessions().await.is_empty(),
        "no session may appear in the session manager"
    );
    // The coordinator worker rolls the pending binding back asynchronously;
    // while it is still visible, the rejected id must show no launch
    // provenance, no PTY map entry, and no output task.
    let pending_ids = session_mgr
        .read()
        .await
        .aggregate_snapshot()
        .await
        .pending_ids;
    for id in &pending_ids {
        assert!(
            crate::pty::spawn_diagnostics::record_for(*id).is_none(),
            "no launch provenance may be recorded for the rejected session"
        );
        assert!(
            !pty_mgr.lock().unwrap().has_session(*id),
            "no PTY map entry may exist for the rejected session"
        );
        assert!(
            pty_mgr.lock().unwrap().get_pty_size(*id).is_none(),
            "no output task may be attached for the rejected session"
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let pending_empty = session_mgr
            .read()
            .await
            .aggregate_snapshot()
            .await
            .pending_ids
            .is_empty();
        if pending_empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pending session/metadata record may survive the rejection"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    close_test_coordinator(&app).await;
}

#[tokio::test]
async fn test_resolve_blocking_menu_command() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let session =
        create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy()).await;
    let session_id = Uuid::parse_str(&session.id).unwrap();

    let menu_guard = app.state::<Arc<crate::pty::menu_guard::MenuGuard>>();
    let pi_entries = crate::config::settings::default_blocking_menus_for_command("pi");
    let menu_rows = vec![crate::pty::watchers::frame::LogicalRow {
        start: 0,
        end: 0,
        text: "Trust project folder?".to_string(),
    }];

    // Evaluate to trigger blocked state
    let eval = menu_guard.evaluate_logical_rows(session_id, &menu_rows, &pi_entries);
    assert!(eval.is_blocked);
    assert!(menu_guard.is_blocked(session_id));

    // Set blocked menu in session manager
    session_mgr
        .read()
        .await
        .set_blocked_menu(
            session_id,
            eval.matched_notification.unwrap(),
            chrono::Utc::now(),
        )
        .await;

    let events = capture_session_lifecycle(&app);

    // Invoke resolve_blocking_menu
    let res = super::resolve_blocking_menu(
        app.handle().clone(),
        app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
        app.state::<Arc<crate::pty::menu_guard::MenuGuard>>(),
        session.id.clone(),
    )
    .await;
    assert!(res.is_ok());

    // Verify menu_guard is no longer blocked for this session
    assert!(!menu_guard.is_blocked(session_id));

    // Verify session manager communication is cleared
    let stored = session_mgr
        .read()
        .await
        .get_session(session_id)
        .await
        .unwrap();
    assert!(stored.communication.is_none());

    // Verify session_communication_changed event was emitted
    let mut found_comm_event = false;
    while let Ok((event, payload)) = events.try_recv() {
        if event == "session_communication_changed" {
            assert!(payload.contains(&session.id));
            assert!(payload.contains("\"communication\":null"));
            found_comm_event = true;
            break;
        }
    }
    assert!(
        found_comm_event,
        "must emit session_communication_changed event"
    );

    close_test_coordinator(&app).await;
}

/// #1682 - the smallest app the stamp path resolves state from: the
/// `SessionManager` for `get_last_agent_message`, and the substantive-input
/// tracker for gate 2. Deliberately manages no `WsBroadcaster`, so
/// `broadcast_all_r` reduces to `tauri::Emitter::emit` and a `listen_any`
/// listener sees the event. `session_test_app_with_store` is not extended:
/// it is shared by more than thirty tests and manages no tracker.
fn agent_stamp_test_app(
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(session_mgr)
        .manage(crate::pty::input_activity::new_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build agent stamp test app")
}

/// #1682 - a local channel over the stamp event. Not `capture_session_lifecycle`,
/// which is shared and scoped to four other event names.
fn capture_agent_message_events(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> std::sync::mpsc::Receiver<String> {
    use tauri::Listener;

    let (sender, receiver) = std::sync::mpsc::channel();
    app.listen_any("session_agent_message", move |event| {
        let _ = sender.send(event.payload().to_string());
    });
    receiver
}

/// #1682 - the per-instance file the stamp is written into: since #2816
/// (C3) the state file, never the tracked `config.json` beside it.
fn stamp_config_path(cwd: &std::path::Path) -> PathBuf {
    cwd.join(crate::config::agent_local_dir_name())
        .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// #2816 (C3) - a suppressed write leaves neither the state file nor the
/// tracked `config.json` it used to create.
fn no_stamp_file(cwd: &std::path::Path) -> bool {
    let state = stamp_config_path(cwd);
    !state.exists() && !state.with_file_name("config.json").exists()
}

/// #1682 - `tooling.lastAgentMessageAt` read straight off the file, so the
/// assertion does not go through the same reader the command uses.
fn stored_stamp(cwd: &std::path::Path) -> Option<String> {
    // #2816 (C3) - the stamp must never be in the tracked file.
    let tracked = stamp_config_path(cwd).with_file_name("config.json");
    if let Ok(raw) = std::fs::read_to_string(&tracked) {
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("tracked json");
        assert!(
            parsed["tooling"].get("lastAgentMessageAt").is_none(),
            "the stamp is still tracked in {}",
            tracked.display()
        );
    }
    let raw = std::fs::read_to_string(stamp_config_path(cwd)).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed["tooling"]["lastAgentMessageAt"]
        .as_str()
        .map(|s| s.to_string())
}

/// #1682 - one session with the `agent_id` and `working_directory` the caller
/// needs, armed or not.
async fn stamp_test_session(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    cwd: &std::path::Path,
    agent_id: Option<String>,
    arm: bool,
) -> Uuid {
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            "codex".into(),
            vec![],
            cwd.to_str().expect("utf-8 temp path").to_string(),
            agent_id,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create the stamp test session");
    if arm {
        mgr.arm_agent_turn(session.id).await;
    }
    session.id
}

/// #1682 - assert exactly one `session_agent_message` arrived for `id`
/// carrying `at`, and nothing else.
fn assert_one_event(events: &std::sync::mpsc::Receiver<String>, id: Uuid, at: &str, context: &str) {
    let payload = events
        .try_recv()
        .unwrap_or_else(|e| panic!("{context}: expected one event, got {e}"));
    assert!(
        payload.contains(&id.to_string()),
        "{context}: event must carry the session id, got {payload}"
    );
    assert!(
        payload.contains(at),
        "{context}: event must carry {at}, got {payload}"
    );
    assert!(
        events.try_recv().is_err(),
        "{context}: exactly one event, not more"
    );
}

#[tokio::test]
async fn get_last_agent_message_binds_the_stored_value_and_every_unavailable_case() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    // Unknown session.
    assert_eq!(
        super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            Uuid::new_v4().to_string()
        )
        .await,
        Ok(None)
    );

    // Malformed uuid.
    assert!(
        super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            "not-a-uuid".to_string()
        )
        .await
        .is_err(),
        "a malformed uuid is an Err, not Ok(None)"
    );

    // Known session with no coding agent.
    let plain_dir = tempfile::tempdir().expect("tempdir");
    let plain = stamp_test_session(&session_mgr, plain_dir.path(), None, false).await;
    assert_eq!(
        super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            plain.to_string()
        )
        .await,
        Ok(None)
    );

    // Agent session whose replica carries no config.json yet.
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let agent = stamp_test_session(
        &session_mgr,
        agent_dir.path(),
        Some("codex".to_string()),
        false,
    )
    .await;
    assert_eq!(
        super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            agent.to_string()
        )
        .await,
        Ok(None)
    );

    // The control: without it every assertion above passes on a function
    // that always returns `Ok(None)`.
    assert_eq!(
        crate::config::agent_config::set_last_agent_message_at(
            agent_dir.path().to_str().expect("utf-8 temp path"),
            "2026-08-31T21:29:07Z",
        ),
        Ok(true)
    );
    assert_eq!(
        super::get_last_agent_message(
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
            agent.to_string()
        )
        .await,
        Ok(Some("2026-08-31T21:29:07Z".to_string()))
    );
}

#[tokio::test]
async fn record_agent_turn_completed_writes_and_emits_only_for_an_armed_agent_session() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);

    // Armed, with a coding agent: the stamp lands and is announced.
    let dir = tempfile::tempdir().expect("tempdir");
    let id = stamp_test_session(&session_mgr, dir.path(), Some("codex".to_string()), true).await;
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None).await;
    let first = stored_stamp(dir.path()).expect("the first edge must write the stamp");
    let first_parsed =
        chrono::DateTime::parse_from_rfc3339(&first).expect("the stored value is RFC3339");
    assert_one_event(&events, id, &first, "first edge");

    // A later edge advances it, and announces the new value.
    tokio::time::sleep(Duration::from_millis(2)).await;
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None).await;
    let second = stored_stamp(dir.path()).expect("the second edge must write the stamp");
    let second_parsed =
        chrono::DateTime::parse_from_rfc3339(&second).expect("the stored value is RFC3339");
    assert!(
        second_parsed > first_parsed,
        "the second edge must be strictly later: {first} -> {second}"
    );
    assert_one_event(&events, id, &second, "second edge");

    // Negative control 1: an agent session nothing ever armed.
    let unarmed_dir = tempfile::tempdir().expect("tempdir");
    let unarmed = stamp_test_session(
        &session_mgr,
        unarmed_dir.path(),
        Some("codex".to_string()),
        false,
    )
    .await;
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, unarmed, None)
        .await;
    assert!(
        no_stamp_file(unarmed_dir.path()),
        "an unarmed session must not create a config.json"
    );
    assert!(
        events.try_recv().is_err(),
        "an unarmed session emits nothing"
    );

    // Negative control 2: armed, but a plain shell with no coding agent.
    let plain_dir = tempfile::tempdir().expect("tempdir");
    let plain = stamp_test_session(&session_mgr, plain_dir.path(), None, true).await;
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, plain, None).await;
    assert!(
        no_stamp_file(plain_dir.path()),
        "a session with no agent_id must not create a config.json"
    );
    assert!(
        events.try_recv().is_err(),
        "a session with no agent_id emits nothing"
    );
}

#[tokio::test]
async fn record_agent_turn_completed_gate_2_skips_only_recent_input() {
    use crate::pty::input_activity::{SubstantiveInputState, USER_WRITE_STAMP_WINDOW};

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);
    let tracker = app.state::<SubstantiveInputState>().inner().clone();

    let dir = tempfile::tempdir().expect("tempdir");
    let id = stamp_test_session(&session_mgr, dir.path(), Some("codex".to_string()), true).await;

    // (a) A half-typed line that just arrived suppresses the stamp.
    tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .feed(id, b"hel");
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None).await;
    assert!(
        no_stamp_file(dir.path()),
        "recent unsubmitted input must suppress the write"
    );
    assert!(
        events.try_recv().is_err(),
        "recent unsubmitted input emits nothing"
    );

    // (b) The age half. The flag is still set and nothing was submitted, but
    // the keystroke is now older than the window, so it no longer suppresses.
    // This is the leg that goes red if gate 2 is written unaged.
    tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, id, None).await;
    let aged = stored_stamp(dir.path()).expect("aged input must not suppress the write");
    chrono::DateTime::parse_from_rfc3339(&aged).expect("the stored value is RFC3339");
    assert_one_event(&events, id, &aged, "aged pending input");

    // (c) The control for (a): a submitted line clears `pending_nonspace`,
    // so the stamp lands. Without it, (a) stays green on a no-op function.
    let submitted_dir = tempfile::tempdir().expect("tempdir");
    let submitted = stamp_test_session(
        &session_mgr,
        submitted_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    {
        let mut guard = tracker.lock().unwrap_or_else(|e| e.into_inner());
        guard.feed(submitted, b"hel");
        guard.feed(submitted, b"\r");
    }
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, submitted, None)
        .await;
    let cleared = stored_stamp(submitted_dir.path()).expect("a submitted line must not suppress");
    chrono::DateTime::parse_from_rfc3339(&cleared).expect("the stored value is RFC3339");
    assert_one_event(&events, submitted, &cleared, "submitted line");
}

#[tokio::test]
async fn record_agent_turn_completed_gate_1_skips_only_a_recent_control_write() {
    use crate::pty::input_activity::USER_WRITE_STAMP_WINDOW;

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = agent_stamp_test_app(Arc::clone(&session_mgr));
    let events = capture_agent_message_events(&app);

    // No input is ever fed to the tracker, so gate 2 is open throughout and
    // `control_write_age` is the only thing under test. The age is a
    // parameter, so no detector fixture is needed.

    // Recent control write: suppressed.
    let recent_dir = tempfile::tempdir().expect("tempdir");
    let recent = stamp_test_session(
        &session_mgr,
        recent_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        recent,
        Some(Duration::from_millis(100)),
    )
    .await;
    assert!(
        no_stamp_file(recent_dir.path()),
        "a control write inside the window must suppress the write"
    );
    assert!(
        events.try_recv().is_err(),
        "a control write inside the window emits nothing"
    );

    // Control 1: a mark older than the window does not suppress. This is the
    // leg that goes red if any `Some(_)` suppresses.
    let aged_dir = tempfile::tempdir().expect("tempdir");
    let aged = stamp_test_session(
        &session_mgr,
        aged_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::record_agent_turn_completed(
        app.handle(),
        &*session_mgr.read().await,
        aged,
        Some(USER_WRITE_STAMP_WINDOW + Duration::from_secs(1)),
    )
    .await;
    let aged_at = stored_stamp(aged_dir.path()).expect("an aged mark must not suppress");
    chrono::DateTime::parse_from_rfc3339(&aged_at).expect("the stored value is RFC3339");
    assert_one_event(&events, aged, &aged_at, "aged control write");

    // Control 2: no mark stands at all.
    let none_dir = tempfile::tempdir().expect("tempdir");
    let none = stamp_test_session(
        &session_mgr,
        none_dir.path(),
        Some("codex".to_string()),
        true,
    )
    .await;
    super::record_agent_turn_completed(app.handle(), &*session_mgr.read().await, none, None).await;
    let none_at = stored_stamp(none_dir.path()).expect("no mark must not suppress");
    chrono::DateTime::parse_from_rfc3339(&none_at).expect("the stored value is RFC3339");
    assert_one_event(&events, none, &none_at, "no control write");
}

// (#1793) Phase 1b: restart auto-resume wake policy and target selection.

fn restart_resume_info(id: &str, status: SessionStatus, waiting_for_input: bool) -> SessionInfo {
    SessionInfo {
        id: id.to_string(),
        name: "s".to_string(),
        shell: "claude".to_string(),
        shell_args: Vec::new(),
        backend_kind: SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-09-07T00:00:00Z".to_string(),
        working_directory: "/w".to_string(),
        status,
        waiting_for_input,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".to_string(),
        agent_kind: None,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

fn restart_resume_target(session_id: Uuid, name: &str) -> RestartResumeTarget {
    RestartResumeTarget {
        session_id,
        name: name.to_string(),
        working_directory: "/w".to_string(),
        prompt: ".".to_string(),
        collected_at: std::time::Instant::now(),
    }
}

#[test]
fn restart_resume_prompt_for_truth_table() {
    let settings = AppSettings::default();
    let orchestrator = settings.restart_resume_orchestrator_prompt.clone();
    for is_coordinator in [false, true] {
        for start_fresh_on_restore in [false, true] {
            for persisted_working in [false, true] {
                let got = restart_resume_prompt_for(
                    &settings,
                    is_coordinator,
                    start_fresh_on_restore,
                    persisted_working,
                );
                let expected = if start_fresh_on_restore || !persisted_working {
                    None
                } else if is_coordinator {
                    Some(orchestrator.as_str())
                } else {
                    Some(".")
                };
                assert_eq!(
                        got, expected,
                        "is_coordinator={is_coordinator} start_fresh_on_restore={start_fresh_on_restore} persisted_working={persisted_working}"
                    );
            }
        }
    }
}

#[test]
fn restart_resume_prompt_for_returns_none_for_an_empty_class_prompt() {
    let settings = AppSettings {
        restart_resume_orchestrator_prompt: String::new(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, true, false, true),
        None
    );
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        Some(".")
    );

    let settings = AppSettings {
        restart_resume_agent_prompt: String::new(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        None
    );
    assert_eq!(
        restart_resume_prompt_for(&settings, true, false, true),
        Some(settings.restart_resume_orchestrator_prompt.as_str())
    );
}

#[test]
fn restart_resume_prompt_for_keeps_a_whitespace_only_prompt() {
    let settings = AppSettings {
        restart_resume_agent_prompt: " ".to_string(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        Some(" ")
    );
}

#[test]
fn restart_resume_session_is_ready_truth_table() {
    assert!(!restart_resume_session_is_ready(None));
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
        SessionStatus::Exited(0),
    ] {
        let not_waiting = restart_resume_info("id", status.clone(), false);
        assert!(
            !restart_resume_session_is_ready(Some(&not_waiting)),
            "waiting_for_input=false must never be ready, status={status:?}"
        );
    }
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
    ] {
        let waiting = restart_resume_info("id", status.clone(), true);
        assert!(
            restart_resume_session_is_ready(Some(&waiting)),
            "status={status:?} must be ready when waiting_for_input"
        );
    }
    let exited = restart_resume_info("id", SessionStatus::Exited(0), true);
    assert!(!restart_resume_session_is_ready(Some(&exited)));
}

#[test]
fn partition_restart_resume_ready_requires_both_conjuncts() {
    let id = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &id.to_string(),
        SessionStatus::Running,
        true,
    )];

    let (remaining, ready) =
        partition_restart_resume_ready(vec![restart_resume_target(id, "a")], &live, |_target| true);
    assert_eq!(ready.len(), 1);
    assert!(remaining.is_empty());

    let (remaining, ready) =
        partition_restart_resume_ready(vec![restart_resume_target(id, "a")], &live, |_target| {
            false
        });
    assert!(
        ready.is_empty(),
        "a session that has printed nothing must not be selected on waiting_for_input alone"
    );
    assert_eq!(remaining.len(), 1);
}

#[test]
fn partition_restart_resume_ready_removes_what_it_selects() {
    let first = Uuid::new_v4();
    let middle = Uuid::new_v4();
    let last = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &middle.to_string(),
        SessionStatus::Running,
        true,
    )];
    let targets = vec![
        restart_resume_target(first, "first"),
        restart_resume_target(middle, "middle"),
        restart_resume_target(last, "last"),
    ];

    let (remaining, ready) = partition_restart_resume_ready(targets, &live, |_target| true);

    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].session_id, middle);
    assert_eq!(
        remaining
            .iter()
            .map(|t| t.session_id)
            .collect::<Vec<Uuid>>(),
        vec![first, last]
    );
    assert_eq!(remaining.len() + ready.len(), 3);
}

#[test]
fn partition_restart_resume_ready_never_offers_the_same_target_twice() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &first.to_string(),
        SessionStatus::Running,
        true,
    )];
    let targets = vec![
        restart_resume_target(first, "first"),
        restart_resume_target(second, "second"),
    ];

    let (remaining, ready) = partition_restart_resume_ready(targets, &live, |_target| true);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].session_id, first);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].session_id, second);

    let (remaining, ready) = partition_restart_resume_ready(remaining, &live, |_target| true);
    assert!(
        ready.is_empty(),
        "an already-injected target must never be offered a second time"
    );
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].session_id, second);
}

// #2482 - the quota engine's registration and retirement at the two chokepoints. The
// watcher engine is built from three no-op fakes: these tests assert REGISTRATION, never
// a tick, so the engine is never started and the fakes are never called.
struct NoopWatcherBackends;
impl crate::pty::watchers::WatcherBackendSource for NoopWatcherBackends {
    fn reader_for(&self, _id: Uuid) -> Option<crate::pty::watchers::SessionFrameReader> {
        None
    }
    fn liveness(&self, _id: Uuid) -> crate::pty::context_scrape::ContextSessionLiveness {
        crate::pty::context_scrape::ContextSessionLiveness::Unavailable
    }
}
struct NoopWatcherPatterns;
impl crate::pty::watchers::WatcherPatternSource for NoopWatcherPatterns {
    fn resolve(
        &self,
    ) -> futures::future::BoxFuture<
        '_,
        Option<HashMap<String, crate::pty::watchers::AgentResolution>>,
    > {
        Box::pin(async { Some(HashMap::new()) })
    }
}
struct NoopWatcherSink;
impl crate::pty::watchers::WatcherEventSink for NoopWatcherSink {
    fn emit(&self, _batch: crate::pty::watchers::WatcherMatchBatch) {}
}

fn watcher_engine() -> Arc<crate::pty::watchers::WatcherEngine> {
    crate::pty::watchers::WatcherEngine::new(
        Arc::new(NoopWatcherBackends),
        Arc::new(NoopWatcherPatterns),
        Arc::new(NoopWatcherSink),
        Arc::new(crate::pty::watchers::history::WatcherHistory::default()),
    )
}

fn quota_app(
    quota: &crate::pty::agent_quota::test_support::QuotaHarness,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(Arc::clone(&quota.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app")
}

#[test]
fn register_session_samplers_registers_the_quota_engine_when_managed() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();

    super::register_session_samplers(app.handle(), id, Some("claude".to_string()));

    assert!(quota.engine.is_session_registered(id));
}

#[test]
fn register_session_samplers_with_no_agent_registers_no_quota_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();

    super::register_session_samplers(app.handle(), id, None);

    assert!(!quota.engine.is_session_registered(id));
}

#[test]
fn register_session_samplers_does_not_panic_when_the_quota_engine_is_unmanaged() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    super::register_session_samplers(app.handle(), Uuid::new_v4(), Some("claude".to_string()));
    super::purge_session_side_state(app.handle(), Uuid::new_v4());
}

#[test]
fn purge_session_side_state_retires_the_quota_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();
    super::register_session_samplers(app.handle(), id, Some("claude".to_string()));
    assert!(quota.engine.is_session_registered(id));

    super::purge_session_side_state(app.handle(), id);

    assert!(!quota.engine.is_session_registered(id));
}

/// A regression net on the `agent_id` clone chain: a quota consumer inserted beside the
/// watcher block is exactly how the watcher registration would stop receiving an id.
#[test]
fn register_session_samplers_still_registers_the_watcher_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let watchers = watcher_engine();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&quota.engine))
        .manage(Arc::clone(&watchers))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");
    let id = Uuid::new_v4();

    super::register_session_samplers(app.handle(), id, Some("claude".to_string()));

    assert!(watchers.is_session_registered(id));
    assert!(quota.engine.is_session_registered(id));
}

// #2434 - restart and drift read one matched selection.

fn route_agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

/// One local `codex` agent with enabled cells `A` (`--a`) and `B` (`--b`).
fn route_settings() -> AppSettings {
    let mut settings = AppSettings {
        agents: vec![route_agent("codex", "Codex", "codex")],
        ..AppSettings::default()
    };
    for (letter, command) in [("A", "--a"), ("B", "--b")] {
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                letter.to_string(),
                ProfileCellConfig {
                    id_hash: None,
                    id_hash_debug: None,
                    preflight: None,
                    preflight_timeout_seconds: None,
                    enabled: true,
                    command: command.to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
    }
    settings
}

fn route_digest(settings: &AppSettings, letter: &str) -> String {
    crate::config::agent_command::cell_identity(
        &settings.agents[0],
        &settings.coding_agent_profiles.profiles_by_agent["codex"][letter],
    )
}

/// Merge `tooling` into `dir/config.json`, keeping every other key.
fn merge_tooling(dir: &std::path::Path, tooling: serde_json::Value) {
    let path = dir.join("config.json");
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let entry = value
        .as_object_mut()
        .unwrap()
        .entry("tooling")
        .or_insert_with(|| serde_json::json!({}));
    for (key, v) in tooling.as_object().unwrap() {
        entry
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), v.clone());
    }
    std::fs::write(&path, value.to_string()).unwrap();
}

struct RouteHarness {
    app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<crate::pty::manager::PtyManager>>,
    backend: Arc<ScriptedSpawnBackend>,
}

impl RouteHarness {
    fn new(settings: AppSettings) -> Self {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(settings, Arc::clone(&manager), Arc::clone(&pty));
        Self {
            app,
            manager,
            pty,
            backend,
        }
    }

    /// A live session recorded with `agent_id` and the saved `shell`/`args`.
    async fn live(
        &self,
        cwd: &std::path::Path,
        agent_id: Option<&str>,
        shell: &str,
        args: &[&str],
    ) -> Uuid {
        let info = super::create_session_inner(
            self.app.handle(),
            &self.manager,
            &self.pty,
            shell.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
            cwd.to_string_lossy().into_owned(),
            Some("route fixture".to_string()),
            agent_id.map(str::to_string),
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
        .expect("create route fixture session");
        Uuid::parse_str(&info.id).unwrap()
    }

    async fn restart(
        &self,
        id: Uuid,
        requested_profile: Option<&str>,
    ) -> Result<SessionInfo, String> {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        super::restart_session_inner_with_intent(
            self.app.handle(),
            &self.manager,
            &self.pty,
            settings.inner(),
            id,
            None,
            requested_profile.map(str::to_string),
            Some(true),
            true,
            crate::session::selection::TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    }

    async fn close(self) {
        close_test_coordinator(&self.app).await;
    }
}

/// A foreign `currentCodingAgent` whose descriptor names local cell `B` as `A`.
fn moved_letter_reference(dir: &std::path::Path, settings: &AppSettings) {
    merge_tooling(
        dir,
        serde_json::json!({
            "profile": "A",
            "currentCodingAgent": "their-codex",
            "codingAgents": {"their-codex": {
                "app": "Theirs",
                "identity": {"A": route_digest(settings, "B")}
            }}
        }),
    );
}

#[tokio::test]
async fn restart_spawns_the_matched_letter_over_a_pinned_replica_letter() {
    let fixture = super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.agent_id.as_deref(), Some("codex"));
    assert!(
        restarted.shell_args.iter().any(|a| a == "--b"),
        "{:?}",
        restarted.shell_args
    );
    assert!(
        !restarted.shell_args.iter().any(|a| a == "--a"),
        "{:?}",
        restarted.shell_args
    );
    assert_eq!(restarted.effective_profile.as_deref(), Some("B"));
    h.close().await;
}

#[tokio::test]
async fn restart_populates_the_match_carrier_fields() {
    let fixture = super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.match_tier.as_deref(), Some("hash"));
    assert_eq!(restarted.original_profile_letter.as_deref(), Some("A"));
    let wire = serde_json::to_value(&restarted).unwrap();
    assert_eq!(wire["matchTier"], "hash");
    assert_eq!(wire["originalProfileLetter"], "A");
    h.close().await;
}

#[tokio::test]
async fn a_matched_restart_records_the_original_request() {
    let fixture = super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.requested_profile.as_deref(), Some("A"));
    assert_eq!(restarted.effective_profile.as_deref(), Some("B"));
    assert_eq!(restarted.original_profile_letter.as_deref(), Some("A"));
    h.close().await;
}

#[tokio::test]
async fn drift_mirrors_a_moved_letter() {
    let fixture = super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings.clone());
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(
        restarted.profile_content_hash.as_deref(),
        Some(route_digest(&settings, "B").as_str())
    );
    assert!(
        !compute_profile_outdated(&settings, &restarted),
        "a matched B must not raise a drift badge against the pinned A"
    );
    h.close().await;
}

#[tokio::test]
async fn restart_with_no_coding_agent_restarts_the_saved_shell() {
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(AppSettings::default());
    let old = h.live(temp.path(), None, "test-shell", &["--keep"]).await;

    let restarted = h.restart(old, None).await.expect("plain shell restart");
    assert_eq!(restarted.shell, "test-shell");
    assert_eq!(restarted.shell_args, vec!["--keep".to_string()]);
    assert_eq!(restarted.agent_id, None);
    assert_eq!(restarted.match_tier, None);
    h.close().await;
}

#[tokio::test]
async fn restart_returns_an_error_on_no_match() {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({
            "currentCodingAgent": "foreign",
            "codingAgents": {"foreign": {"app": "Gemini", "command": "gemini"}}
        }),
    );
    let h = RouteHarness::new(route_settings());
    // C19: actual completed missing-UUID entries release their root capacity.
    // This exceeds the 64-entry admission bound through the real outer core,
    // worker response, sidecar/envelope release and independent publication.
    for _ in 0..65 {
        assert_eq!(
            h.restart(Uuid::new_v4(), None).await.unwrap_err(),
            "Session not found"
        );
    }
    let old = h.live(temp.path(), Some("codex"), "codex", &[]).await;

    // Actual prerequisite errors also release completed C19 entries. If
    // they remained ambiguous, this would exhaust the same 64-entry root.
    for _ in 0..65 {
        let err = h.restart(old, None).await.unwrap_err();
        assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
        assert!(err.contains("'foreign'"), "{err}");
    }
    // No fallback to the stored `codex`, and no teardown.
    let row = h
        .manager
        .read()
        .await
        .get_session(old)
        .await
        .expect("row survives");
    assert!(!matches!(row.status, SessionStatus::Exited(_)));
    assert!(h.backend.has_session(old), "the old PTY is untouched");
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 1);
    h.close().await;
}

#[test]
fn drift_is_false_for_an_unresolved_reference() {
    let temp = tempfile::tempdir().unwrap();
    let settings = route_settings();
    let mut info = make_info(
        &temp.path().to_string_lossy(),
        Some("codex"),
        None,
        Some("stale".to_string()),
    );
    // Control: with no foreign reference, a stale loaded hash IS drift.
    assert!(compute_profile_outdated(&settings, &info));
    merge_tooling(
        temp.path(),
        serde_json::json!({
            "currentCodingAgent": "foreign",
            "codingAgents": {"foreign": {"app": "Gemini"}}
        }),
    );
    assert!(!compute_profile_outdated(&settings, &info));
    // `NoAgent` raises none either.
    info.agent_id = None;
    assert!(!compute_profile_outdated(&settings, &info));
}

#[test]
fn root_agent_chain_is_unchanged() {
    // D5: the root agent keeps its id-only chain; a descriptor-matchable
    // foreign id is NOT routed through the matcher.
    let settings = test_settings();
    let (_, _, agent_id, _) =
        resolve_root_agent_command(&settings, Some("their-codex"), Some("codex")).unwrap();
    assert_eq!(
        agent_id.as_deref(),
        Some("codex"),
        "falls to lastCodingAgent"
    );
    let (_, _, agent_id, _) =
        resolve_root_agent_command(&settings, Some("their-codex"), Some("gone")).unwrap();
    assert_eq!(
        agent_id.as_deref(),
        Some("claude"),
        "then to the first agent"
    );
    let (_, _, agent_id, _) = resolve_root_agent_command(&settings, Some("claude"), None).unwrap();
    assert_eq!(agent_id.as_deref(), Some("claude"), "a valid request first");
}

fn isolated_codex_settings(id: &str) -> (AppSettings, PathBuf) {
    let mut agent = route_agent(id, "Codex", "codex");
    agent.isolated_home = true;
    let settings = AppSettings {
        agents: vec![agent],
        ..AppSettings::default()
    };
    let home =
        crate::config::agent_command::resolve_agent_spawn_command(&settings, id, None, None, false)
            .unwrap()
            .effective_codex_home
            .expect("isolated CODEX_HOME");
    (settings, home)
}

#[tokio::test]
async fn restart_prepares_the_spawn_before_teardown() {
    let id = format!("codex-2434-{}", Uuid::new_v4());
    let (settings, home) = isolated_codex_settings(&id);
    let _ = std::fs::remove_dir_all(&home);
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(settings);
    let old = h.live(temp.path(), Some(&id), "codex", &[]).await;
    assert!(!home.exists());

    h.restart(old, None).await.expect("restart");
    assert!(
        home.is_dir(),
        "the restart prepared the isolated CODEX_HOME"
    );
    std::fs::remove_dir_all(&home).unwrap();
    h.close().await;
}

#[tokio::test]
async fn a_failed_preparation_keeps_the_old_session() {
    let id = format!("codex-2434-{}", Uuid::new_v4());
    let (settings, home) = isolated_codex_settings(&id);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.parent().unwrap()).unwrap();
    // A regular FILE at the directory path: `create_dir_all` fails for real.
    std::fs::write(&home, "not a directory").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(settings);
    let old = h.live(temp.path(), Some(&id), "codex", &[]).await;

    let err = h.restart(old, None).await.unwrap_err();
    assert!(
        err.contains("Failed to create isolated CODEX_HOME"),
        "{err}"
    );
    let row = h
        .manager
        .read()
        .await
        .get_session(old)
        .await
        .expect("row survives");
    assert!(!matches!(row.status, SessionStatus::Exited(_)));
    assert!(h.backend.has_session(old), "never torn down");
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 1);
    std::fs::remove_file(&home).unwrap();
    h.close().await;
}

#[tokio::test]
async fn tier_zero_without_a_request_keeps_the_origin_default() {
    // Origin default `B` (the replica's matrix), then an agent-level default.
    for use_origin in [true, false] {
        let fixture = super::co_managed_tests::room_fixture();
        let cwd = fixture.coordinator_path();
        let mut settings = route_settings();
        if use_origin {
            let matrix = cwd
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("_agent_coordinator");
            merge_tooling(&matrix, serde_json::json!({"defaultProfile": "B"}));
        } else {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .insert("coordinator".to_string(), "B".to_string());
        }
        assert!(matches!(
            super::matched_selection(&settings, &cwd.to_string_lossy(), None, Some("codex"), None),
            super::MatchedSelection::Agent {
                profile_letter: None,
                authoritative: false,
                tier: None,
                ..
            }
        ));
        let h = RouteHarness::new(settings.clone());
        let old = h.live(cwd, Some("codex"), "codex", &[]).await;
        let restarted = h.restart(old, None).await.expect("restart");
        assert_eq!(
            restarted.effective_profile.as_deref(),
            Some("B"),
            "origin={use_origin}"
        );
        assert!(restarted.shell_args.iter().any(|a| a == "--b"));
        assert!(
            !compute_profile_outdated(&settings, &restarted),
            "drift agrees"
        );
        h.close().await;
    }
}

#[tokio::test]
async fn a_matched_reference_without_a_request_keeps_the_default() {
    let label = serde_json::json!({"app": "CODEX"});
    let command = serde_json::json!({"app": "Other", "command": "codex"});
    for (descriptor, tier) in [
        (
            label,
            crate::config::agent_command::MatchTier::LabelAndLetter,
        ),
        (
            command,
            crate::config::agent_command::MatchTier::CommandAndLetter,
        ),
    ] {
        let fixture = super::co_managed_tests::room_fixture();
        let cwd = fixture.coordinator_path();
        let mut settings = route_settings();
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert("coordinator".to_string(), "B".to_string());
        merge_tooling(
            cwd,
            serde_json::json!({
                "currentCodingAgent": "foreign",
                "codingAgents": {"foreign": descriptor}
            }),
        );
        assert_eq!(
            super::matched_selection(&settings, &cwd.to_string_lossy(), None, Some("codex"), None),
            super::MatchedSelection::Agent {
                agent_id: "codex".to_string(),
                profile_letter: None,
                authoritative: false,
                tier: Some(tier),
                original_letter: None,
            }
        );
        let h = RouteHarness::new(settings.clone());
        let old = h.live(cwd, Some("codex"), "codex", &[]).await;
        let restarted = h.restart(old, None).await.expect("restart");
        assert_eq!(
            restarted.effective_profile.as_deref(),
            Some("B"),
            "{tier:?}"
        );
        assert_eq!(restarted.match_tier.as_deref(), Some(tier.as_wire_str()));
        assert!(
            !compute_profile_outdated(&settings, &restarted),
            "drift agrees"
        );
        h.close().await;
    }
}

fn selection_for(
    tooling: serde_json::Value,
    requested: Option<&str>,
    stored: Option<&str>,
) -> (tempfile::TempDir, super::MatchedSelection) {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(temp.path(), tooling);
    let selection = super::matched_selection(
        &route_settings(),
        &temp.path().to_string_lossy(),
        requested,
        stored,
        None,
    );
    (temp, selection)
}

async fn assert_saved_shell_restart(current: &str) {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": current}),
    );
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), None, "test-shell", &["--keep"]).await;
    let restarted = h.restart(old, None).await.expect("plain shell restart");
    assert_eq!(restarted.shell, "test-shell");
    assert_eq!(restarted.shell_args, vec!["--keep".to_string()]);
    assert_eq!(restarted.agent_id, None);
    h.close().await;
}

#[tokio::test]
async fn an_empty_current_coding_agent_restarts_the_saved_shell() {
    let (_t, selection) = selection_for(serde_json::json!({"currentCodingAgent": ""}), None, None);
    assert_eq!(selection, super::MatchedSelection::NoAgent);
    assert_saved_shell_restart("").await;
}

#[tokio::test]
async fn a_whitespace_current_coding_agent_is_absent() {
    let (_t, selection) =
        selection_for(serde_json::json!({"currentCodingAgent": "   "}), None, None);
    assert_eq!(selection, super::MatchedSelection::NoAgent);
    // With a stored agent the blank id must not become a reference either.
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "   "}),
        None,
        Some("codex"),
    );
    assert!(
        matches!(selection, super::MatchedSelection::Agent { ref agent_id, .. } if agent_id == "codex"),
        "{selection:?}"
    );
    assert_saved_shell_restart("   ").await;
}

#[test]
fn an_empty_current_coding_agent_is_absent_even_with_a_stored_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": ""}),
        None,
        Some("codex"),
    );
    assert!(
        matches!(selection, super::MatchedSelection::Agent { ref agent_id, tier: None, .. } if agent_id == "codex"),
        "{selection:?}"
    );
}

#[tokio::test]
async fn a_plain_shell_ignores_an_unresolvable_current_coding_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
        None,
        None,
    );
    assert_eq!(selection, super::MatchedSelection::NoAgent);
    assert_saved_shell_restart("ghost-agent").await;
}

#[tokio::test]
async fn an_unresolvable_reference_still_fails_a_session_that_has_an_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
        None,
        Some("codex"),
    );
    assert_eq!(
        selection,
        super::MatchedSelection::Unresolved {
            reference: "ghost-agent".to_string()
        }
    );
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
    );
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), Some("codex"), "codex", &[]).await;
    let err = h.restart(old, None).await.unwrap_err();
    assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
    assert!(h.backend.has_session(old), "no teardown");
    h.close().await;
}

#[tokio::test]
async fn an_explicit_unresolvable_request_fails_without_a_stored_agent() {
    let (_t, selection) = selection_for(serde_json::json!({}), Some("ghost-agent"), None);
    assert_eq!(
        selection,
        super::MatchedSelection::Unresolved {
            reference: "ghost-agent".to_string()
        }
    );
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), None, "test-shell", &[]).await;
    let settings = h.app.state::<crate::config::settings::SettingsState>();
    let err = super::restart_session_inner_with_intent(
        h.app.handle(),
        &h.manager,
        &h.pty,
        settings.inner(),
        old,
        Some("ghost-agent".to_string()),
        None,
        Some(true),
        true,
        crate::session::selection::TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
    assert!(h.backend.has_session(old), "no teardown");
    h.close().await;
}

#[test]
fn drift_is_false_for_a_plain_shell_with_a_stale_reference() {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
    );
    let info = make_info(
        &temp.path().to_string_lossy(),
        None,
        None,
        Some("stale".to_string()),
    );
    assert!(!compute_profile_outdated(&route_settings(), &info));
    assert_eq!(
        super::matched_selection(
            &route_settings(),
            &temp.path().to_string_lossy(),
            None,
            None,
            None
        ),
        super::MatchedSelection::NoAgent
    );
}

// #2433 descriptor persistence

fn descriptor_settings() -> AppSettings {
    let agent = |id: &str, command: &str| AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    let cell = |enabled: bool, command: &str| ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled,
        command: command.to_string(),
        env: BTreeMap::new(),
        notes: String::new(),
    };
    let mut settings = AppSettings {
        agents: vec![agent("claude", "Claude --Foo"), agent("codex", "codex")],
        ..AppSettings::default()
    };
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "claude".to_string(),
        BTreeMap::from([
            ("A".to_string(), cell(true, "--a")),
            ("B".to_string(), cell(false, "--b")),
            ("C".to_string(), cell(true, "")),
        ]),
    );
    settings
}

#[test]
fn descriptor_identity_skips_disabled_cells() {
    let settings = descriptor_settings();
    let agent = &settings.agents[0];
    let cells = &settings.coding_agent_profiles.profiles_by_agent["claude"];
    let (command, identity) = coding_agent_descriptor("claude", &settings).unwrap();
    assert_eq!(
        command,
        crate::config::agent_command::canonical_command_text("Claude --Foo")
    );
    let expected: BTreeMap<String, String> = ["A", "C"]
        .iter()
        .map(|l| {
            (
                l.to_string(),
                crate::config::agent_command::cell_identity(agent, &cells[*l]),
            )
        })
        .collect();
    assert_eq!(identity, expected);

    let (_, none) = coding_agent_descriptor("codex", &settings).unwrap();
    assert!(none.is_empty());
    assert!(coding_agent_descriptor("gone", &settings).is_none());
}

/// #2816 (C3) - the state file beside the tracked `path`, after asserting
/// that the tracked file keeps none of the descriptor's state keys.
fn read_state_beside(path: &std::path::Path) -> serde_json::Value {
    let tracked = read_json(path);
    for key in ["lastCodingAgent", "codingAgents"] {
        assert!(tracked["tooling"].get(key).is_none(), "{key} in {path:?}");
    }
    read_json(&path.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Seeds both config files the spawn-time write opens; returns their paths.
fn seed_both_configs(repo: &std::path::Path, seed: &serde_json::Value) -> [PathBuf; 2] {
    let instance = repo.join(crate::config::agent_local_dir_name());
    std::fs::create_dir_all(&instance).unwrap();
    let paths = [instance.join("config.json"), repo.join("config.json")];
    for path in &paths {
        std::fs::write(path, serde_json::to_string(seed).unwrap()).unwrap();
    }
    paths
}

#[test]
fn spawn_writes_the_descriptor_for_the_launched_agent_only() {
    let settings = descriptor_settings();
    let temp = tempfile::tempdir().unwrap();
    let other = serde_json::json!({"app": "codex", "lastUsed": "2026-01-01T00:00:00+00:00"});
    let paths = seed_both_configs(
        temp.path(),
        &serde_json::json!({"tooling": {"codingAgents": {"codex": other}}}),
    );

    // Expected values derived here, not through the code under test.
    let agent = &settings.agents[0];
    let cells = &settings.coding_agent_profiles.profiles_by_agent["claude"];
    let digest = |l: &str| crate::config::agent_command::cell_identity(agent, &cells[l]);
    let expected_identity = serde_json::json!({"A": digest("A"), "C": digest("C")});
    let expected_command = crate::config::agent_command::canonical_command_text("Claude --Foo");

    let write = prepare_last_coding_agent_write("claude", None, &settings);
    save_last_coding_agent(&temp.path().to_string_lossy(), "sid", &write).unwrap();

    for path in &paths {
        let value = read_state_beside(path);
        let agents = &value["tooling"]["codingAgents"];
        assert_eq!(value["tooling"]["lastCodingAgent"], "claude", "{path:?}");
        assert_eq!(agents["claude"]["app"], "claude", "{path:?}");
        assert_eq!(agents["claude"]["acSessionId"], "sid", "{path:?}");
        assert_eq!(
            agents["claude"]["command"],
            serde_json::json!(expected_command)
        );
        assert_eq!(agents["claude"]["identity"], expected_identity, "{path:?}");
        assert_eq!(agents["codex"], other, "{path:?}");
    }
}

#[test]
fn spawn_with_an_unknown_agent_keeps_the_existing_descriptor() {
    let settings = descriptor_settings();
    let temp = tempfile::tempdir().unwrap();
    let good = serde_json::json!({
        "app": "Gone", "lastUsed": "2026-01-01T00:00:00+00:00",
        "command": "gone --x", "identity": {"A": "aa"}
    });
    let paths = seed_both_configs(
        temp.path(),
        &serde_json::json!({"tooling": {"codingAgents": {"gone": good}}}),
    );

    let write = prepare_last_coding_agent_write("gone", None, &settings);
    save_last_coding_agent(&temp.path().to_string_lossy(), "sid", &write).unwrap();

    for path in &paths {
        let value = read_state_beside(path);
        let entry = &value["tooling"]["codingAgents"]["gone"];
        assert_eq!(entry["command"], "gone --x", "{path:?}");
        assert_eq!(
            entry["identity"],
            serde_json::json!({"A": "aa"}),
            "{path:?}"
        );
        assert_eq!(entry["app"], "Unknown", "{path:?}");
        assert_eq!(entry["acSessionId"], "sid", "{path:?}");
    }
}

/// #2716 (B3, option C) E33: a restart of a session whose stored agent id is
/// set refuses under the mark instead of coming back as a plain terminal.
#[tokio::test]
async fn a_restart_refuses_instead_of_becoming_a_plain_terminal() {
    use tauri::Manager;

    let cwd = tempfile::tempdir().unwrap();
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
    let cwd_text = cwd.path().to_string_lossy().to_string();
    let spawn =
        super::build_configured_agent_spawn_for_cwd(&test_settings(), "claude", &cwd_text, None)
            .unwrap()
            .expect("claude is configured");
    let old = super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd_text.clone(),
        Some("agent fixture".to_string()),
        Some("claude".to_string()),
        Some(spawn.trusted_agent_label.clone()),
        true,
        Vec::new(),
        true,
        Some(spawn),
        Some(super::ResolvedAgentHostShell {
            program: "test-shell".to_string(),
            args: Vec::new(),
        }),
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("create the agent session");
    assert_eq!(
        old.agent_id.as_deref(),
        Some("claude"),
        "fixture: no stored agent"
    );
    let old_id = Uuid::parse_str(&old.id).unwrap();
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    let sidecar = cwd.path().join("agents.30.instance.no-git.json");
    let settings = app.state::<crate::config::settings::SettingsState>();
    settings.write().await.agents_layer = crate::config::settings::AgentsLayerState::Unreadable {
        path: sidecar.clone(),
        reason: "test".to_string(),
    };

    let error = super::restart_session_inner_with_activation(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        settings.inner(),
        old_id,
        None,
        None,
        Some(true),
        true,
    )
    .await
    .expect_err("the restart must refuse under the mark");

    assert!(
        error.contains(&crate::config::settings::agents_unreadable_session_error(
            &sidecar
        )),
        "{error}"
    );
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        1,
        "a plain-terminal replacement was spawned"
    );
    close_test_coordinator(&app).await;
}

/// #2716 (B3, option C) E32: entry point 1. `create_session` reaches
/// `build_configured_agent_spawn_for_cwd(&cfg, aid, ..)?` before anything
/// else; with the mark set by the real loader it answers notice 3, naming
/// the agents file and without option S's fourth sentence.
#[test]
fn create_session_refuses_under_the_mark() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join(SETTINGS_FILE_NAME);
    std::fs::write(
            &settings_path,
            r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "agents": [{"id":"claude","label":"Claude Code","command":"claude","color":"#d97706"}]
            }"##,
        )
        .unwrap();
    let sidecar = temp.path().join(AGENTS_INSTANCE_FILE_NAME);
    std::fs::write(&sidecar, "{ not json").unwrap();
    let cfg = crate::config::settings::load_settings_from_path(&settings_path);
    assert_eq!(cfg.agents_layer.unreadable_path(), Some(sidecar.as_path()));
    assert!(cfg.agents.iter().any(|agent| agent.id == "claude"));
    let cwd = temp.path().to_string_lossy().into_owned();

    let error = super::build_configured_agent_spawn_for_cwd(&cfg, "claude", &cwd, None)
        .expect_err("entry point 1 must refuse under the mark");

    assert_eq!(
        error,
        crate::config::settings::agents_unreadable_session_error(&sidecar)
    );
    assert!(error.contains(&sidecar.display().to_string()));
    assert!(!error.contains("changes to your sessions are not saved"));

    // Repaired: the same fixture builds the agent spawn.
    std::fs::remove_file(&sidecar).unwrap();
    let cfg = crate::config::settings::load_settings_from_path(&settings_path);
    assert!(cfg.agents_layer.unreadable_path().is_none());
    assert!(
        super::build_configured_agent_spawn_for_cwd(&cfg, "claude", &cwd, None)
            .unwrap()
            .is_some()
    );
}
// ---------------------------------------------------------------- #2786 C1
// E14: `currentCodingAgent` and the descriptor come from one merged
// snapshot. The state file's descriptor (`new`) must win over `old`.
#[test]
fn matched_selection_takes_the_descriptor_from_the_state_file() {
    use crate::config::agent_config::load_probe_harness as h;
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new");
    let selection = super::matched_selection(
        &h::snapshot_settings(),
        &tmp.path().to_string_lossy(),
        None,
        None,
        None,
    );
    assert!(
        matches!(&selection, super::MatchedSelection::Agent { agent_id, .. } if agent_id == "via-new"),
        "{selection:?}"
    );
}

// E15: unarmed parent, armed dedicated child, REAL caller. Base at fd32c052
// was already GREEN, n=1, so this is a regression row: it holds n=1 now
// that the caller's own direct `config.json` read is gone and one loader
// snapshot serves both the id and the descriptor.
#[test]
fn matched_selection_loads_once_from_its_own_dir() {
    let out = crate::config::agent_config::load_probe_harness::expect_child_pass(
        "commands::session::tests::inner_matched_selection_loads_once",
    );
    assert!(out.contains("MEASURED-LOADS n=1"), "{out}");
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_matched_selection_loads_once() {
    use crate::config::agent_config::load_probe::Observation;
    use crate::config::agent_config::load_probe_harness as h;
    let settings = h::snapshot_settings();
    let call = |dir: &std::path::Path, want: &str| {
        let obs = Observation::open();
        let selection =
            super::matched_selection(&settings, &dir.to_string_lossy(), None, None, None);
        println!("REACHED-CALLER selection={selection:?}");
        let loads = obs.loads();
        println!("MEASURED-LOADS n={} {:?}", loads.len(), loads);
        let expected = vec![dir.to_path_buf()];
        assert_eq!(
            loads, expected,
            "recorded {loads:?}, expected exactly {expected:?}"
        );
        assert!(
            matches!(&selection, super::MatchedSelection::Agent { agent_id, .. } if agent_id == want),
            "want {want}: {selection:?}"
        );
    };

    // E14's fixture: exactly one load, of this directory.

    println!("LEG main");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new");
    call(tmp.path(), "via-new");

    // Unkeyed cache leg: two fresh directories with different values.

    println!("LEG unkeyed");
    for command in ["new-a", "new-b"] {
        let tmp = tempfile::tempdir().expect("tempdir");
        h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", command);
        call(tmp.path(), &format!("via-{command}"));
    }

    // Keyed cache leg: the SAME directory, state rewritten between calls.

    println!("LEG keyed");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new-a");
    call(tmp.path(), "via-new-a");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new-b");
    call(tmp.path(), "via-new-b");
}
