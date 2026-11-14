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
    messages: Arc<crate::config::injected_messages::MessageWarnings>,
) -> tauri::async_runtime::JoinHandle<()> {
    let lifecycle_owner = Arc::clone(&settings_state);
    let watchdog_settings = Arc::clone(&settings_state);
    let lifecycle_observer = Arc::clone(&restore_observer_barrier);
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
            RestoreContext {
                session_mgr_clone,
                pty_mgr_clone,
                settings_state_clone,
                settings_snapshot,
                restore_transaction_for_task,
                shutdown_for_body,
                restart_resume_targets_for_body,
                teams,
                setting_on,
            }
            .run(persisted)
            .await;
        })
    });
    let continuation = Box::new(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            RestoreObservers {
                app,
                restore_observer_barrier,
                idle_detector,
                restart_resume_targets,
                shutdown,
                git_watcher,
                discovery_branch_watcher,
                resource_monitor,
                watchdog_settings,
                selection_coordinator,
                pty_mgr,
                messages,
                loop_scheduler,
                non_stop_state,
                ui_automation_state,
            }
            .start()
            .await;
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

fn capture_binding_seeds(
    persisted: &mut [sessions_persistence::PersistedSession],
    settings_snapshot: &config::settings::AppSettings,
) {
    for ps in persisted {
        if let Err(error) = ps.capture_binding_seed(settings_snapshot) {
            log::error!(
                "[restore] binding seed unavailable for '{}': {}",
                ps.name,
                error
            );
        }
    }
}

fn session_wake_decision(
    ps: &sessions_persistence::PersistedSession,
    teams: &[crate::config::teams::DiscoveredTeam],
    archived_roots: &[String],
    setting_on: bool,
    settings_snapshot: &config::settings::AppSettings,
) -> (String, bool, bool, bool, bool, bool) {
    let agent_name = crate::config::teams::agent_fqn_from_path(&ps.working_directory);
    let live_is_coord = crate::config::teams::is_any_coordinator(&agent_name, teams);
    // (#630) Backstop a transient empty discover_teams() with the snapshot's
    // persisted is_coordinator so a real coordinator is not silently downgraded
    // to "deferred" when project paths were not ready at cold start.
    let is_coord = resolve_is_coord_for_restore(live_is_coord, teams.is_empty(), ps.is_coordinator);
    let archived_session = sessions_persistence::is_under_normalized_archived_roots(
        &ps.working_directory,
        archived_roots,
    );
    let persisted_working =
        crate::session::session::persisted_is_working(ps.status.as_ref(), ps.waiting_for_input);
    let wake = restore_session_should_wake(
        crate::restore::RestoreWakeOptions {
            settings: crate::restore::RestoreWakeSettings {
                setting_on,
                resume_agents_on: settings_snapshot.restart_resume_wake_working_agents,
            },
            archived_session,
            is_coord,
            persisted_working,
        },
        ps.status.as_ref(),
    );
    (
        agent_name,
        live_is_coord,
        is_coord,
        archived_session,
        persisted_working,
        wake,
    )
}

fn collect_restart_resume_target(
    session_id: &str,
    ps: &sessions_persistence::PersistedSession,
    settings_snapshot: &config::settings::AppSettings,
    is_coord: bool,
    persisted_working: bool,
    restart_resume_targets_for_body: &Mutex<Vec<commands::session::RestartResumeTarget>>,
) {
    if let Ok(uuid) = uuid::Uuid::parse_str(session_id) {
        let prompt = commands::session::restart_resume_prompt_for(
            settings_snapshot,
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(target);
        }
    }
}

fn report_failed_sessions(failed_recoverable: &[sessions_persistence::PersistedSession]) {
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
}

fn root_spawn_recipe(
    ps: &sessions_persistence::PersistedSession,
    settings_snapshot: &config::settings::AppSettings,
    root_path: &str,
    failed_recoverable: &mut Vec<sessions_persistence::PersistedSession>,
) -> (
    bool,
    Option<crate::config::agent_command::AgentSpawnCommand>,
    Option<crate::pty::backend::ResolvedAgentHostShell>,
) {
    let mut rebuild_failed = false;
    let resolved_spawn =
        if ps.agent_id.is_some() && settings_snapshot.agents_layer.unreadable_path().is_some() {
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
                settings_snapshot,
                aid,
                root_path,
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
    (rebuild_failed, resolved_spawn, resolved_agent_host_shell)
}

fn session_spawn_recipe(
    ps: &sessions_persistence::PersistedSession,
    settings_snapshot: &config::settings::AppSettings,
    failed_recoverable: &mut Vec<sessions_persistence::PersistedSession>,
) -> Result<Option<crate::config::agent_command::AgentSpawnCommand>, ()> {
    let resolved_spawn =
        if ps.agent_id.is_some() && settings_snapshot.agents_layer.unreadable_path().is_some() {
            // #2716 (B3, 4.5b option S): skip without pushing to failed_recoverable.
            log::warn!(
                "Skipping restore of '{}': the agents layer is unreadable",
                ps.name
            );
            return Err(());
        } else if let Some(aid) = ps.agent_id.as_deref() {
            match commands::session::build_configured_agent_spawn_for_cwd(
                settings_snapshot,
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
                    return Err(());
                }
            }
        } else {
            None
        };
    Ok(resolved_spawn)
}

mod roots;
mod sessions;

struct RestoreContext {
    session_mgr_clone: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr_clone: Arc<Mutex<PtyManager>>,
    settings_state_clone: SettingsState,
    settings_snapshot: config::settings::AppSettings,
    restore_transaction_for_task: crate::session::selection::SelectionTransaction<tauri::Wry>,
    shutdown_for_body: ShutdownSignal,
    restart_resume_targets_for_body: Arc<Mutex<Vec<commands::session::RestartResumeTarget>>>,
    teams: Vec<crate::config::teams::DiscoveredTeam>,
    setting_on: bool,
}

#[derive(Default)]
struct RestoreProgress {
    active_id: Option<String>,
    failed_recoverable: Vec<sessions_persistence::PersistedSession>,
    n_woken: usize,
    n_deferred: usize,
}

impl RestoreContext {
    async fn run(self, persisted: Result<Vec<sessions_persistence::PersistedSession>, String>) {
        let mut persisted = match persisted {
            Ok(persisted) => persisted,
            Err(error) => {
                log::error!(
                    "[restore] Project state unavailable; automatic restore skipped: {error}"
                );
                return;
            }
        };
        {
            let manager = self.session_mgr_clone.read().await;
            sessions_persistence::initialize_prompt_retention(&manager).await;
            sessions_persistence::block_duplicate_restore_prompt_domains(&manager, &persisted)
                .await;
        }
        capture_binding_seeds(&mut persisted, &self.settings_snapshot);
        let mut progress = RestoreProgress::default();
        if !self.restore_root(&persisted, &mut progress).await {
            return;
        }
        let archived_roots = sessions_persistence::normalize_project_roots(
            &self.settings_snapshot.archived_project_paths,
        );
        for ps in &persisted {
            if ps.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
            {
                continue;
            }

            if let Err(error) = ps.binding_metadata() {
                log::error!("[restore] invalid binding for '{}': {}", ps.name, error);
                progress.failed_recoverable.push(ps.clone());
                continue;
            }
            // 14.4 - exit during restore: stop waking once shutdown
            // is underway. The row rides in progress.failed_recoverable so
            // persist_merging_failed keeps it on disk for next boot
            // (a bare break would drop it from sessions.json).
            if self.shutdown_for_body.token().is_cancelled() {
                progress.failed_recoverable.push(ps.clone());
                break;
            }

            self.restore_session(ps, &archived_roots, &mut progress)
                .await;
        }
        // #248 Grinch Z5 — load-bearing summary line. Replaces the per-session
        // info noise demoted to debug above. Must be emitted BEFORE the post-loop
        // active-switch block so the restore-decision summary is grouped with
        // the restore log in chronological order, not interleaved with switch events.
        log::info!(
            "[restore] complete — {} woken, {} deferred (setting_on={}, total_evaluated={})",
            progress.n_woken,
            progress.n_deferred,
            self.setting_on,
            persisted.len()
        );

        let persisted_target = progress
            .active_id
            .as_deref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok());
        if let Err(error) = self
            .restore_transaction_for_task
            .restore_selection_inline(persisted_target)
            .await
        {
            log::error!(
                "[restore] final canonical selection failed target={persisted_target:?}: {error}"
            );
        }

        // Persist restored sessions + failed-but-recoverable entries
        let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> =
            self.session_mgr_clone.read().await;
        sessions_persistence::persist_merging_failed(&mgr, &progress.failed_recoverable).await;

        report_failed_sessions(&progress.failed_recoverable);
    }
}

struct RestoreObservers {
    app: tauri::AppHandle,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    idle_detector: Arc<IdleDetector>,
    restart_resume_targets: Arc<Mutex<Vec<commands::session::RestartResumeTarget>>>,
    shutdown: ShutdownSignal,
    git_watcher: Arc<GitWatcher>,
    discovery_branch_watcher: Arc<DiscoveryBranchWatcher>,
    resource_monitor: Arc<resource_monitor::ResourceMonitorState>,
    watchdog_settings: SettingsState,
    selection_coordinator: crate::session::selection::SelectionCoordinator,
    pty_mgr: Arc<Mutex<PtyManager>>,
    messages: Arc<crate::config::injected_messages::MessageWarnings>,
    loop_scheduler: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ui_automation_state: crate::testability::ui_automation::UiAutomationState,
}

impl RestoreObservers {
    async fn start(self) {
        let Self {
            app,
            restore_observer_barrier,
            idle_detector,
            restart_resume_targets,
            shutdown,
            git_watcher,
            discovery_branch_watcher,
            resource_monitor,
            watchdog_settings,
            selection_coordinator,
            pty_mgr,
            messages,
            loop_scheduler,
            non_stop_state,
            ui_automation_state,
        } = self;
        // These observers mutate session metadata or persistence directly.
        // Start them only after restore has completed, which is stricter than
        // merely placing restore first and prevents an intermediate snapshot.
        if let Err(e) = restore_observer_barrier.start("idle", || {
            idle_detector.start(shutdown.clone());
        }) {
            log::error!("[restore] observer 'idle' start failed: {e}");
        }

        // (#1793) After the idle observer start, because that observer
        // maintains the `waiting_for_input` this polls, and before the rest
        // of the tail so a later tail panic cannot lose it. This single
        // drain is the whole once-per-app-start latch.
        let restart_resume_batch: Vec<commands::session::RestartResumeTarget> =
            restart_resume_targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
                .collect();
        if !restart_resume_batch.is_empty() {
            let resume_app = app.app_handle().clone();
            let resume_shutdown = shutdown.clone();
            tauri::async_runtime::spawn(async move {
                tokio::select! {
                    biased;
                    () = resume_shutdown.token().cancelled() => {}
                    () = commands::session::run_restart_resume(
                        &resume_app,
                        restart_resume_batch,
                    ) => {}
                }
            });
        }

        if let Err(e) = restore_observer_barrier.start("git", || {
            git_watcher.start(shutdown.clone());
        }) {
            log::error!("[restore] observer 'git' start failed: {e}");
        }

        if let Err(e) = restore_observer_barrier.start("discovery", || {
            discovery_branch_watcher.start(shutdown.clone());
        }) {
            log::error!("[restore] observer 'discovery' start failed: {e}");
        }

        resource_monitor::watchdog::start(
            (*resource_monitor).clone(),
            watchdog_settings,
            selection_coordinator.clone(),
            shutdown.clone(),
        );
        pty_mgr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .start_container_pending_reaper(shutdown.clone());

        app.state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
            .start_artifact_cleanup(&shutdown);
        let mailbox_poller = phone::mailbox::MailboxPoller::new(messages);
        mailbox_poller.start(app.app_handle().clone(), shutdown.clone());
        loop_scheduler
            .clone()
            .start(app.app_handle().clone(), shutdown.clone());
        crate::commands::session::selection_executor::start_response_close_polling(
            app.app_handle().clone(),
            shutdown.clone(),
        );
        crate::session::auto_close::AutoClose::start(app.clone(), shutdown.clone());
        crate::loops::non_stop_watchdog::start(
            app.app_handle().clone(),
            non_stop_state.clone(),
            shutdown.clone(),
        );
        ui_automation_state.start(app.app_handle().clone(), shutdown.clone());
    }
}
