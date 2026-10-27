//! Native restore of persisted sessions at startup.

use std::sync::{Arc, Mutex};

use crate::commands::ac_discovery::DiscoveryBranchWatcher;
use crate::commands::session::{
    attach_persisted_telegram_if_configured, execute_destroy_transaction, DestroyRequest,
    DestructionSource,
};
use crate::config::sessions_persistence;
use crate::config::settings::SettingsState;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::TrustedCreateIntent;
use crate::shutdown::ShutdownSignal;
use tauri::Manager;

use crate::app_state::RestoreInProgress;
use crate::restore::{
    resolve_is_coord_for_restore, restore_session_should_become_active,
    restore_session_should_wake, should_auto_create_root_agent_on_first_restore,
    should_wake_root_agent_on_restore, skip_auto_resume_for_restore, spawn_restore_startup,
    RestoreObserverStartBarrier,
};
use crate::{commands, config, loops, phone, resource_monitor};

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_native_restore_startup(
    app: tauri::AppHandle,
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    restore_transaction: crate::session::selection::SelectionTransaction<tauri::Wry>,
    restore_flag: Arc<RestoreInProgress>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings_state: SettingsState,
    settings_snapshot: config::settings::AppSettings,
    persisted: Result<Vec<sessions_persistence::PersistedSession>, String>,
    teams: Vec<crate::config::teams::DiscoveredTeam>,
    setting_on: bool,
    idle_detector: Arc<IdleDetector>,
    git_watcher: Arc<GitWatcher>,
    discovery_branch_watcher: Arc<DiscoveryBranchWatcher>,
    resource_monitor: Arc<resource_monitor::ResourceMonitorState>,
    selection_coordinator: crate::session::selection::SelectionCoordinator,
    loop_scheduler: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ui_automation_state: crate::testability::ui_automation::UiAutomationState,
    shutdown: ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    let lifecycle_owner = Arc::clone(&settings_state);
    let lifecycle_observer = Arc::clone(&restore_observer_barrier);
    let app_handle = app.clone();
    let pty_mgr_clone = pty_mgr.clone();
    let session_mgr_clone = session_mgr;
    let settings_state_clone = settings_state;
    let restore_transaction_for_task = restore_transaction;
    let shutdown_for_body = shutdown.clone();
    let restart_resume_targets: Arc<Mutex<Vec<commands::session::RestartResumeTarget>>> =
        Arc::new(Mutex::new(Vec::new()));
    let restart_resume_targets_for_body = Arc::clone(&restart_resume_targets);
    let body = Box::new(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut persisted = match persisted {
                Ok(persisted) => persisted,
                Err(error) => {
                    log::error!(
                        "[restore] Project state unavailable; automatic restore skipped: {}",
                        error
                    );
                    return;
                }
            };
            {
                let manager = session_mgr_clone.read().await;
                sessions_persistence::initialize_prompt_retention(&manager).await;
                sessions_persistence::block_duplicate_restore_prompt_domains(&manager, &persisted)
                    .await;
            }
            for ps in &mut persisted {
                if let Err(error) = ps.capture_binding_seed(&settings_snapshot) {
                    log::error!(
                        "[restore] binding seed unavailable for '{}': {}",
                        ps.name,
                        error
                    );
                }
            }
            let mut active_id = None;
            let mut failed_recoverable: Vec<sessions_persistence::PersistedSession> = Vec::new();

            // #248 Grinch Z5 — count outcomes for the end-of-restore summary line.
            let mut n_woken: usize = 0;
            let mut n_deferred: usize = 0;

            let root_agent_path = match crate::config::root_agent::ensure_root_agent_dir() {
                Ok(path) => Some(path),
                Err(e) => {
                    log::error!(
                        "[root-agent] Failed to provision root agent during restore: {}",
                        e
                    );
                    None
                }
            };
            let root_ps = persisted
                .iter()
                .find(|ps| {
                    ps.is_root_agent
                        || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
                })
                .cloned();

            if let Some(root_path) = root_agent_path.clone() {
                if shutdown_for_body.token().is_cancelled() {
                    // 14.4 - exit during restore: never spawn a root PTY
                    // once shutdown is underway (it would be orphaned until
                    // the next boot's container-orphan cleanup). Keep the
                    // row for next boot via the failed_recoverable merge.
                    if let Some(ps) = root_ps.as_ref() {
                        failed_recoverable.push(ps.clone());
                    }
                } else {
                    match root_ps.as_ref() {
                        None => {
                            let last_coding_agent =
                                crate::config::root_agent::read_last_coding_agent(&root_path);
                            let should_auto_create = {
                                let cfg = match settings_state_clone.current_read().await {
                                    Ok(cfg) => cfg,
                                    Err(error) => {
                                        log::debug!("[restore] settings unavailable: {error}");
                                        return;
                                    }
                                };
                                should_auto_create_root_agent_on_first_restore(
                                    &cfg,
                                    last_coding_agent.as_deref(),
                                )
                            };

                            if should_auto_create {
                                match commands::session::execute_root_transaction(
                                    &restore_transaction_for_task,
                                    commands::session::RootJobRequest {
                                        requested_agent_id: None,
                                        requested_profile: None,
                                        skip_auto_resume_for_new_session: true,
                                        intent: TrustedCreateIntent::Background,
                                        select_after: false,
                                    },
                                )
                                .await
                                {
                                    Ok(_) => n_woken += 1,
                                    Err(e) => log::error!(
                                        "[root-agent] Failed to auto-create root session: {}",
                                        e
                                    ),
                                }
                            } else {
                                log::info!(
                                    "[root-agent] Skipping startup auto-create: no resolvable \
                                        coding agent is configured"
                                );
                            }
                        }
                        Some(ps) if ps.binding_metadata().is_err() => {
                            log::error!("[restore] ambiguous Root binding; preserving recipe");
                            failed_recoverable.push(ps.clone());
                        }
                        Some(ps) if should_wake_root_agent_on_restore(ps.status.as_ref()) => {
                            let existing_root = {
                                let mgr = session_mgr_clone.read().await;
                                mgr.list_sessions().await.into_iter().find(|s| {
                                    s.is_root_agent
                                        || crate::config::root_agent::is_root_agent_path(
                                            &s.working_directory,
                                        )
                                })
                            };
                            let mut should_create = true;
                            if let Some(existing) = existing_root {
                                if matches!(
                                    existing.status,
                                    crate::session::session::SessionStatus::Exited(_)
                                ) {
                                    if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                        let stale_destroy = execute_destroy_transaction(
                                            &restore_transaction_for_task,
                                            DestroyRequest {
                                                ids: vec![uuid],
                                                source: DestructionSource::BackgroundCleanup,
                                                force_destroy_root: true,
                                            },
                                        )
                                        .await
                                        .and_then(|outcome| {
                                            outcome.succeeded(uuid).then_some(()).ok_or_else(|| {
                                                "stale dormant Root was not destroyed".to_string()
                                            })
                                        });
                                        if let Err(e) = stale_destroy {
                                            log::warn!(
                                                "[root-agent] Failed to force-destroy stale \
                                                    dormant root during restore: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    if ps.was_active {
                                        active_id = Some(existing.id.clone());
                                    }
                                    n_woken += 1;
                                    if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                        attach_persisted_telegram_if_configured(
                                            &app_handle,
                                            uuid,
                                            ps.telegram_bot_id.as_deref(),
                                        )
                                        .await;
                                        commands::session::seed_root_last_prompt(
                                            &app_handle,
                                            &session_mgr_clone,
                                            uuid,
                                            ps,
                                        )
                                        .await;
                                    }
                                    should_create = false;
                                }
                            }
                            if should_create {
                                let mut rebuild_failed = false;
                                let resolved_spawn = if ps.agent_id.is_some()
                                    && settings_snapshot.agents_layer.unreadable_path().is_some()
                                {
                                    // #2716 (B3, 4.5b option S): skip, and push nothing to
                                    // failed_recoverable, so the row keeps its wake inputs;
                                    // save_sessions_to_dir suppresses the rewrite.
                                    log::warn!(
                                        "[root-agent] Skipping restore of '{}': the agents \
                                            layer is unreadable",
                                        ps.name
                                    );
                                    rebuild_failed = true;
                                    None
                                } else if let Some(aid) = ps.agent_id.as_deref() {
                                    match commands::session::build_configured_agent_spawn_for_cwd(
                                        &settings_snapshot,
                                        aid,
                                        &root_path,
                                        ps.requested_profile.as_deref(),
                                    ) {
                                        Ok(spawn) => spawn,
                                        Err(e) => {
                                            log::error!(
                                                "[root-agent] Failed to rebuild configured \
                                                    agent command for restore '{}': {}",
                                                ps.name,
                                                e
                                            );
                                            failed_recoverable.push(ps.clone());
                                            rebuild_failed = true;
                                            None
                                        }
                                    }
                                } else {
                                    None
                                };
                                // #1271 - the host shell is rebuilt at restore
                                // time from the SAME settings snapshot that
                                // re-resolved the agent command (no persistence:
                                // a persisted shell could pair with a freshly
                                // re-resolved command across a config change).
                                let resolved_agent_host_shell = if resolved_spawn.is_some() {
                                    Some(crate::pty::backend::ResolvedAgentHostShell {
                                        program: settings_snapshot.default_shell.clone(),
                                        args: settings_snapshot.default_shell_args.clone(),
                                    })
                                } else {
                                    None
                                };
                                if !rebuild_failed {
                                    let (shell, shell_args, agent_label) =
                                        if let Some(spawn) = resolved_spawn.as_ref() {
                                            (
                                                spawn.shell.clone(),
                                                spawn.shell_args.clone(),
                                                Some(spawn.trusted_agent_label.clone()),
                                            )
                                        } else {
                                            (
                                                ps.shell.clone(),
                                                ps.shell_args.clone(),
                                                ps.agent_label.clone(),
                                            )
                                        };
                                    match commands::session::create_session_inner_for_restore(
                                        &restore_transaction_for_task,
                                        &session_mgr_clone,
                                        &pty_mgr_clone,
                                        shell,
                                        shell_args,
                                        root_path.clone(),
                                        Some(ps.name.clone()),
                                        ps.agent_id.clone(),
                                        agent_label,
                                        false,
                                        ps.git_repos.clone(),
                                        false,
                                        resolved_spawn,
                                        resolved_agent_host_shell,
                                        // #973 - headless caller: no terminal to measure, keep
                                        // 120x30.
                                        None,
                                        Some(ps.start_fresh_on_restore),
                                        None,
                                        ps.binding_metadata()
                                            .expect("restore binding validated before spawn"),
                                        Some(ps),
                                    )
                                    .await
                                    {
                                        Ok(info) => {
                                            if ps.was_active {
                                                active_id = Some(info.id.clone());
                                            }
                                            n_woken += 1;
                                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                attach_persisted_telegram_if_configured(
                                                    &app_handle,
                                                    uuid,
                                                    ps.telegram_bot_id.as_deref(),
                                                )
                                                .await;
                                            }

                                            if ps.was_detached {
                                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                    {
                                                        let mgr = session_mgr_clone.read().await;
                                                        if let Some(ref geo) = ps.detached_geometry
                                                        {
                                                            mgr.set_detached_geometry(
                                                                uuid,
                                                                geo.clone(),
                                                            )
                                                            .await;
                                                        }
                                                    }

                                                    let detached_result =
                                                    commands::window::execute_detach_transaction(
                                                        &restore_transaction_for_task,
                                                        uuid,
                                                        ps.detached_geometry.clone(),
                                                        true,
                                                    )
                                                    .await;
                                                    if let Err(e) = detached_result {
                                                        log::warn!(
                                                        "[restore] detach_terminal_inner failed \
                                                            for root agent '{}': {} — session \
                                                            stays live (attached)",
                                                        ps.name,
                                                        e
                                                    );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!(
                                            "[root-agent] Failed to restore root session '{}': {}",
                                            ps.name,
                                            e
                                        );
                                            failed_recoverable.push(ps.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Some(ps) => {
                            let existing_root = {
                                let mgr = session_mgr_clone.read().await;
                                mgr.list_sessions().await.into_iter().find(|s| {
                                    s.is_root_agent
                                        || crate::config::root_agent::is_root_agent_path(
                                            &s.working_directory,
                                        )
                                })
                            };
                            if let Some(existing) = existing_root {
                                if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                    let mgr = session_mgr_clone.read().await;
                                    commands::session::preserve_deferred_telegram_intent_if_valid(
                                        &mgr,
                                        &settings_state_clone,
                                        uuid,
                                        &ps.name,
                                        ps.telegram_bot_id.as_deref(),
                                    )
                                    .await;
                                    drop(mgr);
                                    commands::session::seed_root_last_prompt(
                                        &app_handle,
                                        &session_mgr_clone,
                                        uuid,
                                        ps,
                                    )
                                    .await;
                                }
                                if ps.was_active {
                                    active_id = Some(existing.id.clone());
                                }
                                n_deferred += 1;
                            } else {
                                match restore_transaction_for_task
                                    .restore_dormant_inline(
                                        crate::session::selection::DormantRestoreRequest {
                                            persisted: ps.clone(),
                                            working_directory: root_path,
                                            is_coordinator: false,
                                            is_root_agent: true,
                                        },
                                    )
                                    .await
                                {
                                    Ok(info) => {
                                        if ps.was_active {
                                            active_id = Some(info.id);
                                        }
                                        n_deferred += 1;
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "[root-agent] Failed to create dormant root session \
                                                '{}': {}",
                                            ps.name,
                                            e
                                        );
                                        failed_recoverable.push(ps.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            } else if let Some(ps) = root_ps.as_ref() {
                failed_recoverable.push(ps.clone());
            }

            let archived_roots = sessions_persistence::normalize_project_roots(
                &settings_snapshot.archived_project_paths,
            );

            for ps in &persisted {
                if ps.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
                {
                    continue;
                }

                if let Err(error) = ps.binding_metadata() {
                    log::error!("[restore] invalid binding for '{}': {}", ps.name, error);
                    failed_recoverable.push(ps.clone());
                    continue;
                }
                // 14.4 - exit during restore: stop waking once shutdown
                // is underway. The row rides in failed_recoverable so
                // persist_merging_failed keeps it on disk for next boot
                // (a bare break would drop it from sessions.json).
                if shutdown_for_body.token().is_cancelled() {
                    failed_recoverable.push(ps.clone());
                    break;
                }

                // Skip sessions whose CWD no longer exists (permanent failure)
                if !crate::filesystem::directories::restore_cwd_exists(std::path::Path::new(
                    &ps.working_directory,
                )) {
                    log::warn!(
                        "Skipping restore of '{}': CWD '{}' no longer exists",
                        ps.name,
                        ps.working_directory
                    );
                    // §1295 site C (restore-skip): append ONE archive
                    // record BEFORE the `continue`. The restore task is
                    // async and holds no `sessions_save_lock` here, so we
                    // use the locking public variant (S4). The record is
                    // written before the continue unchanged; it does not
                    // touch sessions.json (site C leaves the row's disk
                    // fate to the next persist, §224 G5).
                    let config_dir = crate::config::config_dir();
                    if let Some(config_dir) = config_dir {
                        sessions_persistence::append_orphan_archive_record(
                            &config_dir,
                            "restoreCwdMissing",
                            "archived",
                            ps,
                        )
                        .await;
                    }
                    continue;
                }

                // #248 — decide wake vs defer for this session.
                // §DR2: use `agent_fqn_from_path` so WG replicas get project-precise
                // team membership and coordinator checks. Strict `is_coordinator`
                // (§AR2-strict) requires the FQN to avoid cross-project flag leaks.
                let agent_name = crate::config::teams::agent_fqn_from_path(&ps.working_directory);
                let live_is_coord = crate::config::teams::is_any_coordinator(&agent_name, &teams);
                // (#630) Backstop a transient empty discover_teams() with the snapshot's
                // persisted is_coordinator so a real coordinator is not silently downgraded
                // to "deferred" when project paths were not ready at cold start.
                let is_coord = resolve_is_coord_for_restore(
                    live_is_coord,
                    teams.is_empty(),
                    ps.is_coordinator,
                );
                let archived_session = sessions_persistence::is_under_normalized_archived_roots(
                    &ps.working_directory,
                    &archived_roots,
                );
                let persisted_working = crate::session::session::persisted_is_working(
                    ps.status.as_ref(),
                    ps.waiting_for_input,
                );
                let wake = restore_session_should_wake(
                    archived_session,
                    setting_on,
                    settings_snapshot.restart_resume_wake_working_agents,
                    is_coord,
                    ps.status.as_ref(),
                    persisted_working,
                );

                if !wake {
                    // Defer: create a dormant Session record (no PTY, status = Exited(0)).
                    match restore_transaction_for_task
                        .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                            persisted: ps.clone(),
                            working_directory: ps.working_directory.clone(),
                            is_coordinator: is_coord,
                            is_root_agent: false,
                        })
                        .await
                    {
                        Ok(info) => {
                            // Grinch Z5 — debug, not info: under the new default every
                            // persisted session lands here, and an info-level line per
                            // session creates a "mass defer" wall in startup logs that
                            // looks like an alarm. The end-of-loop info summary below
                            // carries the load-bearing signal.
                            log::debug!(
                                "Deferred session '{}' on startup (agent: {}, is_coord: {}, \
                                    setting: {}, persisted_status: {:?}, was_detached: {})",
                                ps.name,
                                agent_name,
                                is_coord,
                                setting_on,
                                ps.status,
                                ps.was_detached
                            );
                            n_deferred += 1;
                            // Preserve `was_active` for the post-loop active-switch:
                            // a deferred session can still be the persisted-active one.
                            // The post-loop branching (Fix A) ensures `set_active_only`
                            // is used (not `switch_session`), so the dormant status
                            // survives selection.
                            if restore_session_should_become_active(ps.was_active, archived_session)
                            {
                                active_id = Some(info.id);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to create deferred session '{}': {}", ps.name, e);
                            failed_recoverable.push(ps.clone());
                        }
                    }
                    continue;
                }

                // Wake: rebuild configured-agent sessions from the persisted recipe,
                // while custom-shell records keep their materialized shell args.
                let resolved_spawn = if ps.agent_id.is_some()
                    && settings_snapshot.agents_layer.unreadable_path().is_some()
                {
                    // #2716 (B3, 4.5b option S): skip without pushing to failed_recoverable.
                    log::warn!(
                        "Skipping restore of '{}': the agents layer is unreadable",
                        ps.name
                    );
                    continue;
                } else if let Some(aid) = ps.agent_id.as_deref() {
                    match commands::session::build_configured_agent_spawn_for_cwd(
                        &settings_snapshot,
                        aid,
                        &ps.working_directory,
                        ps.requested_profile.as_deref(),
                    ) {
                        Ok(spawn) => spawn,
                        Err(e) => {
                            log::error!(
                                "Failed to rebuild configured agent command for restore '{}': {}",
                                ps.name,
                                e
                            );
                            failed_recoverable.push(ps.clone());
                            continue;
                        }
                    }
                } else {
                    None
                };
                // #1271 - rebuilt at restore time from the SAME settings
                // snapshot that re-resolved the agent command (no
                // persistence, same rationale as the root-agent restore).
                let resolved_agent_host_shell = if resolved_spawn.is_some() {
                    Some(crate::pty::backend::ResolvedAgentHostShell {
                        program: settings_snapshot.default_shell.clone(),
                        args: settings_snapshot.default_shell_args.clone(),
                    })
                } else {
                    None
                };
                let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref()
                {
                    (
                        spawn.shell.clone(),
                        spawn.shell_args.clone(),
                        Some(spawn.trusted_agent_label.clone()),
                    )
                } else {
                    (
                        ps.shell.clone(),
                        ps.shell_args.clone(),
                        ps.agent_label.clone(),
                    )
                };

                // Wake: full PTY restore inside the restore transaction.
                match commands::session::create_session_inner_for_restore(
                    &restore_transaction_for_task,
                    &session_mgr_clone,
                    &pty_mgr_clone,
                    shell,
                    shell_args,
                    ps.working_directory.clone(),
                    Some(ps.name.clone()),
                    ps.agent_id.clone(),
                    agent_label,
                    false, // Persist tooling on restore
                    ps.git_repos.clone(),
                    // (#630/#631) resume unless restarted fresh
                    skip_auto_resume_for_restore(ps.start_fresh_on_restore),
                    resolved_spawn,
                    resolved_agent_host_shell,
                    // #973 - headless caller: no terminal to measure, keep 120x30.
                    None,
                    Some(ps.start_fresh_on_restore),
                    is_coord
                        .then(|| {
                            crate::commands::session::carry_communication_for_restart(
                                ps.communication.clone(),
                                ps.start_fresh_on_restore,
                            )
                        })
                        .flatten(),
                    ps.binding_metadata()
                        .expect("restore binding validated before spawn"),
                    Some(ps),
                )
                .await
                {
                    Ok(info) => {
                        if ps.was_active {
                            active_id = Some(info.id.clone());
                        }
                        n_woken += 1;
                        // (#630/#631) Restore-decision trace (INFO during stabilization).
                        log::info!(
                            "[restore] woke '{}' (is_coord={}, live_is_coord={}, \
                                start_fresh_on_restore={})",
                            ps.name,
                            is_coord,
                            live_is_coord,
                            ps.start_fresh_on_restore
                        );
                        if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                            attach_persisted_telegram_if_configured(
                                &app_handle,
                                uuid,
                                ps.telegram_bot_id.as_deref(),
                            )
                            .await;
                        }

                        // (#1793) Pair the row with the session it just became;
                        // both are in hand, so no correlation key is needed.
                        if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                            let prompt = commands::session::restart_resume_prompt_for(
                                &settings_snapshot,
                                is_coord,
                                ps.start_fresh_on_restore,
                                persisted_working,
                            );
                            if let Some(prompt) = prompt {
                                let target = commands::session::RestartResumeTarget {
                                    session_id: uuid,
                                    name: ps.name.clone(),
                                    working_directory: ps.working_directory.clone(),
                                    prompt: prompt.to_string(),
                                    // (#1793) Stamped HERE, after the PTY was
                                    // spawned and therefore after
                                    // `register_session` seeded the activity
                                    // clock, so "printable output strictly
                                    // after this instant" cannot be satisfied
                                    // by the spawn seed alone.
                                    collected_at: std::time::Instant::now(),
                                };
                                restart_resume_targets_for_body
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .push(target);
                            }
                        }

                        // Phase 3 restore: reconstruct detach state for the live session.
                        // Deferred sessions (handled above with a `continue`) never reach
                        // this branch, so R.9's "skip detached-window spawn for deferred"
                        // guard is enforced structurally by this code path.
                        if ps.was_detached {
                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                // Restore geometry independently. The detach transaction
                                // commits persisted intent only after the window and PTY
                                // rechecks pass.
                                {
                                    let mgr = session_mgr_clone.read().await;
                                    if let Some(ref geo) = ps.detached_geometry {
                                        mgr.set_detached_geometry(uuid, geo.clone()).await;
                                    }
                                }

                                let detached_result = commands::window::execute_detach_transaction(
                                    &restore_transaction_for_task,
                                    uuid,
                                    ps.detached_geometry.clone(),
                                    true,
                                )
                                .await;
                                if let Err(e) = detached_result {
                                    log::warn!(
                                        "[restore] detach_terminal_inner failed for '{}': {} — \
                                            session stays live (attached)",
                                        ps.name,
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to restore session '{}': {}", ps.name, e);
                        // Preserve for next startup attempt (CWD exists, transient failure)
                        failed_recoverable.push(ps.clone());
                    }
                }
            }

            // #248 Grinch Z5 — load-bearing summary line. Replaces the per-session
            // info noise demoted to debug above. Must be emitted BEFORE the post-loop
            // active-switch block so the restore-decision summary is grouped with
            // the restore log in chronological order, not interleaved with switch events.
            log::info!(
                "[restore] complete — {} woken, {} deferred (setting_on={}, total_evaluated={})",
                n_woken,
                n_deferred,
                setting_on,
                persisted.len()
            );

            let persisted_target = active_id
                .as_deref()
                .and_then(|id| uuid::Uuid::parse_str(id).ok());
            if let Err(error) = restore_transaction_for_task
                .restore_selection_inline(persisted_target)
                .await
            {
                log::error!(
                    "[restore] final canonical selection failed target={:?}: {}",
                    persisted_target,
                    error
                );
            }

            // Persist restored sessions + failed-but-recoverable entries
            let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> =
                session_mgr_clone.read().await;
            sessions_persistence::persist_merging_failed(&mgr, &failed_recoverable).await;

            if !failed_recoverable.is_empty() {
                log::warn!(
                    "Session restore: {} sessions failed (preserved for next attempt): {:?}",
                    failed_recoverable.len(),
                    failed_recoverable
                        .iter()
                        .map(|s| &s.name)
                        .collect::<Vec<_>>()
                );
            }
        })
    });
    let continuation = Box::new(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            // These observers mutate session metadata or persistence directly.
            // Start them only after restore has completed, which is stricter than
            // merely placing restore first and prevents an intermediate snapshot.
            if let Err(e) = restore_observer_barrier.start("idle", || {
                idle_detector.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'idle' start failed: {}", e);
            }

            // (#1793) After the idle observer start, because that observer
            // maintains the `waiting_for_input` this polls, and before the rest
            // of the tail so a later tail panic cannot lose it. This single
            // drain is the whole once-per-app-start latch.
            let restart_resume_batch: Vec<commands::session::RestartResumeTarget> =
                restart_resume_targets
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .drain(..)
                    .collect();
            if !restart_resume_batch.is_empty() {
                let resume_app = app.app_handle().clone();
                let resume_shutdown = shutdown.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::select! {
                        biased;
                        _ = resume_shutdown.token().cancelled() => {}
                        _ = commands::session::run_restart_resume(
                            &resume_app,
                            restart_resume_batch,
                        ) => {}
                    }
                });
            }

            if let Err(e) = restore_observer_barrier.start("git", || {
                git_watcher.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'git' start failed: {}", e);
            }

            if let Err(e) = restore_observer_barrier.start("discovery", || {
                discovery_branch_watcher.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'discovery' start failed: {}", e);
            }

            resource_monitor::watchdog::start(
                (*resource_monitor).clone(),
                app.state::<SettingsState>().inner().clone(),
                selection_coordinator.clone(),
                shutdown.clone(),
            );
            pty_mgr
                .lock()
                .unwrap()
                .start_container_pending_reaper(shutdown.clone());

            app.state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
                .start_artifact_cleanup();
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.app_handle().clone(), shutdown.clone());
            loop_scheduler
                .clone()
                .start(app.app_handle().clone(), shutdown.clone());
            crate::commands::session::selection_executor::start_response_close_polling(
                app.app_handle().clone(),
                shutdown.clone(),
            );
            crate::session::auto_close::start(app.app_handle().clone(), shutdown.clone());
            crate::loops::non_stop_watchdog::start(
                app.app_handle().clone(),
                non_stop_state.clone(),
                shutdown.clone(),
            );
            ui_automation_state.start(app.app_handle().clone(), shutdown.clone());
        })
    });
    spawn_restore_startup(
        restore_barrier,
        lifecycle_observer,
        restore_flag,
        lifecycle_owner,
        body,
        continuation,
    )
}
