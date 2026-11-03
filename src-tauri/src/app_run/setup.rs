//! The Tauri `.setup` hook of `run()`: everything that needs the live `App`.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{Emitter, Manager};

use crate::{
    agent_update, api, apply_main_display_state, centered_default_main_geometry,
    codex_activity_factory, commands, config, effective_main_display_state,
    install_container_route_remover, load_startup_sessions, loops,
    normalize_persisted_active_flags, resource_monitor, response_close_wiring_matches,
    sessions_persistence, spawn_co_managed_supervisor, start_native_restore_startup,
    AgentQuotaEngine, ApiServerHandle, ApiServerTask, CoManagedSupervisorHandle, CoManagedTrigger,
    ContextScraper, DiscoveryBranchWatcher, GitWatcher, IdleDetector, OutputSenderMap,
    PersistedActiveFlagNormalization, PtyManager, QuotaRows, QuotaSink, QuotaSources,
    RestoreInProgress, RestoreObserverStartBarrier, ScraperPatterns, ScraperPersist, ScraperRows,
    ScraperSamples, ScraperSink, SessionManager, SettingsState, ShutdownSignal, UpdateCheckState,
    WatcherBackends, WatcherDelivery, WatcherEngine, WatcherPatterns, WatcherSink, WebAccessToken,
    WebServerHandle, WsBroadcaster, WEB_SERVER_START_CANCELLED,
};

mod geometry;
mod servers;
mod window;

pub(super) struct SetupCtx {
    pub(super) owners: crate::AppOwners,
    pub(super) test_window_placement:
        Option<crate::testability::window_placement::TestWindowPlacement>,
    pub(super) app_handle_lock: Arc<OnceLock<tauri::AppHandle>>,
    pub(super) co_managed_supervisor_for_setup: CoManagedSupervisorHandle,
    pub(super) co_managed_triggers: tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>,
    pub(super) response_close_for_setup: crate::pty::manager::ResponseCloseState,
    pub(super) session_mgr_for_git: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) session_mgr_for_git_sweeper: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) session_mgr_for_discovery: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) session_mgr_for_web: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) session_mgr_for_api: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) session_mgr_for_scraper: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) output_senders_for_pty: OutputSenderMap,
    pub(super) idle_detector_for_pty: Arc<IdleDetector>,
    pub(super) idle_detector_for_setup: Arc<IdleDetector>,
    pub(super) broadcaster_for_pty: WsBroadcaster,
    pub(super) broadcaster_for_web: WsBroadcaster,
    pub(super) web_access_token: Arc<WebAccessToken>,
    pub(super) web_token_for_server: Arc<WebAccessToken>,
    pub(super) settings_for_web: SettingsState,
    pub(super) loop_scheduler_for_setup: Arc<loops::scheduler::LoopScheduler>,
    pub(super) non_stop_state_for_setup: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    pub(super) ipc_observer_for_setup: Arc<crate::loops::ipc_observer::IpcObserver>,
    pub(super) update_check_state_for_setup: UpdateCheckState,
    pub(super) agent_update_gate_for_setup: Arc<agent_update::AgentUpdateGate>,
    pub(super) shutdown_for_setup: ShutdownSignal,
    pub(super) selection_coordinator_for_setup: crate::session::selection::SelectionCoordinator,
    pub(super) resource_monitor_for_setup: Arc<resource_monitor::ResourceMonitorState>,
    pub(super) ui_automation_state_for_setup: crate::testability::ui_automation::UiAutomationState,
}
pub(super) fn setup(app: &mut tauri::App, ctx: SetupCtx) -> Result<(), Box<dyn std::error::Error>> {
    let SetupCtx {
        owners: _owners,
        test_window_placement,
        app_handle_lock,
        co_managed_supervisor_for_setup,
        co_managed_triggers,
        response_close_for_setup,
        session_mgr_for_git: git_rows,
        session_mgr_for_git_sweeper: sweep_rows,
        session_mgr_for_discovery: rows,
        session_mgr_for_web,
        session_mgr_for_api,
        session_mgr_for_scraper,
        output_senders_for_pty,
        idle_detector_for_pty,
        idle_detector_for_setup,
        broadcaster_for_pty,
        broadcaster_for_web,
        web_access_token,
        web_token_for_server,
        settings_for_web,
        loop_scheduler_for_setup,
        non_stop_state_for_setup,
        ipc_observer_for_setup,
        update_check_state_for_setup,
        agent_update_gate_for_setup,
        shutdown_for_setup,
        selection_coordinator_for_setup,
        resource_monitor_for_setup,
        ui_automation_state_for_setup,
    } = ctx;
    let messages = _owners.messages;
    // Make AppHandle available to idle detector callbacks
    let _ = app_handle_lock.set(app.handle().clone());

    // #2232 phase 7: start the Co-managed supervisor once the app
    // handle exists. It owns the slot watchers, the idle-edge handling
    // and the effect, all inside existing SCC members.
    spawn_co_managed_supervisor(
        app.handle().clone(),
        co_managed_supervisor_for_setup.clone(),
        co_managed_triggers,
        shutdown_for_setup.token().clone(),
    );

    spawn_screenshot_hotkey_registration(app);

    // #264 — spawn the background task that emits `error_log_event`
    // pings to the UI when ERROR entries are captured. The task runs
    // OUTSIDE the env_logger format closure (see §3.7 / B1). Entries
    // logged before this point stay buffered; the frontend's first
    // `drain_error_logs` call collects them.
    tauri::async_runtime::spawn(crate::logging::spawn_error_emit_task(app.handle().clone()));

    // #271 — seed `<config_dir>/agent-templates/` + README on startup.
    crate::commands::role_templates::ensure_default_templates_dir_at_config();
    // (#621) GC the context-cache: unlink generated *-context-*.md files
    // older than the retention window. Cleans orphans from removed
    // workgroups AND caps the unbounded-growth secondary finding. Robust +
    // self-healing: live agents re-write their cache every launch.
    crate::config::session_context::sweep_context_cache_at_startup();
    let git = _owners.git;
    let (git_watcher, discovery_branch_watcher) = start_git_watchers(
        app,
        git_rows.clone(),
        rows,
        sweep_rows,
        &git,
        &shutdown_for_setup,
    );
    start_remote_activity(
        app,
        git_rows,
        git,
        _owners.remote,
        &shutdown_for_setup,
        &messages,
    );
    let pty_mgr = Arc::new(Mutex::new(PtyManager::with_owners(
        output_senders_for_pty,
        idle_detector_for_pty,
        Arc::clone(&git_watcher),
        Some(broadcaster_for_pty),
        Some(selection_coordinator_for_setup.container_lifecycle_sender()),
        #[cfg(windows)]
        _owners.git_guard.clone(),
        _owners.spawn,
    )));
    prepare_startup_pty(&pty_mgr, &response_close_for_setup);
    app.manage(pty_mgr.clone());
    spawn_startup_agent_updates(app, &agent_update_gate_for_setup);
    let (restore_barrier, restore_observer_barrier, restore_transaction) =
        admit_restore_first(app, &selection_coordinator_for_setup)?;
    start_session_observers(
        app,
        &pty_mgr,
        session_mgr_for_scraper,
        &idle_detector_for_setup,
        &shutdown_for_setup,
        &messages,
    );
    start_menu_guard_and_ipc_observer(
        app,
        &response_close_for_setup,
        &pty_mgr,
        &idle_detector_for_setup,
        &ipc_observer_for_setup,
        &shutdown_for_setup,
    );
    // The authoritative scope of the activity window. `open_watchers_window` writes it
    // before every emit and the window pulls it after subscribing, so a re-scope that
    // races the window's own load is recovered instead of dropped in silence.
    app.manage(commands::window::WatchersScopeState::default());
    register_web_command_dispatcher(
        app,
        &session_mgr_for_web,
        &pty_mgr,
        &settings_for_web,
        &broadcaster_for_web,
    )?;
    servers::start_web_server_if_enabled(
        app,
        servers::WebServerStart {
            settings: settings_for_web,
            web_token: web_token_for_server,
            session_mgr: session_mgr_for_web,
            broadcaster: broadcaster_for_web,
        },
        &pty_mgr,
        &shutdown_for_setup,
        &web_access_token,
    );

    servers::start_api_server_if_enabled(app, &session_mgr_for_api, &pty_mgr, &shutdown_for_setup);

    start_detached_startup_downloads(app, &update_check_state_for_setup);

    provision_startup_messages(messages.as_ref());

    // §224 A.2.5 / G-IMPL-1 — Set restore_in_progress=TRUE BEFORE the
    // mailbox poller starts. The restore task now also owns the root-agent
    // first-start path, so it must run even with no persisted sessions.
    //
    // SEQUENCE-CRITICAL: `MailboxPoller::start()` spawns a tokio worker
    // task that runs its first poll WITHOUT delay (mailbox.rs:200-204)
    // in parallel with the rest of setup() on the main thread. If a
    // close-session message is queued in any outbox at startup, that
    // first poll picks it up. With the flag stuck false, the race-
    // guard wait loop in handle_close_session (§A.2.5,
    // mailbox.rs:1201-1242) is bypassed → status="no_match" instead
    // of "restore_in_progress", AND the A.7 cleanup (mailbox.rs:1293-
    // 1311) drops the failed-recoverable ghosts the restore loop was
    // about to retry. This recreates the exact silent-success bug
    // #224 was filed to fix.
    //
    // Hoisting the flag set above mailbox_poller.start() closes the
    // race window. The restore task spawned below (§A.2.5 RAII guard)
    // is still responsible for clearing the flag when restore
    // completes (or panics).
    //
    // The matching `load_sessions()` call at the original site is
    // removed; `persisted` is reused by the restore task below.
    let restore_settings_snapshot = config::settings::load_settings();
    let persisted = load_normalized_startup_sessions(&restore_settings_snapshot);
    let restore_flag = app.state::<Arc<RestoreInProgress>>().inner().clone();
    restore_flag
        .0
        .store(true, std::sync::atomic::Ordering::SeqCst);

    window::create_main_window(app, test_window_placement.as_ref())?;

    // Restore sessions from last run
    //
    // §224 G-IMPL-1 — `persisted` and `restore_flag` are hoisted above
    // mailbox_poller.start() (see comment block there). `persisted` is
    // reused here; the flag is already TRUE when we enter this block.
    // #1341 - the restore now runs as a spawned runtime task
    // (spawn_restore_startup), never inside a main-thread block_on; the
    // main thread returns to the event loop so the webview can load
    // while the #1327 update gate is pending.
    start_restore(
        app,
        RestoreStart {
            restore_barrier,
            restore_observer_barrier,
            restore_transaction,
            settings_snapshot: restore_settings_snapshot,
            persisted,
            idle_detector: idle_detector_for_setup,
            git_watcher,
            discovery_branch_watcher,
            resource_monitor: resource_monitor_for_setup,
            selection_coordinator: selection_coordinator_for_setup,
            loop_scheduler: loop_scheduler_for_setup,
            non_stop_state: non_stop_state_for_setup,
            ui_automation_state: ui_automation_state_for_setup,
            shutdown: shutdown_for_setup,
            messages,
        },
    );

    Ok(())
}

/// The owned values the startup restore task consumes.
struct RestoreStart {
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: RestoreObserverStartBarrier,
    restore_transaction: crate::session::selection::SelectionTransaction<tauri::Wry>,
    settings_snapshot: config::settings::AppSettings,
    persisted: Result<Vec<sessions_persistence::PersistedSession>, String>,
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
}

fn start_restore(app: &tauri::App, start: RestoreStart) {
    let session_mgr_clone = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings_state_clone = app.state::<SettingsState>().inner().clone();
    let app_handle = app.handle().clone();

    // #248 — read the new setting and always discover teams (the coord check
    // is run for every persisted session, regardless of the setting's value).
    let settings_snapshot = start.settings_snapshot.clone();
    let setting_on = settings_snapshot.restore_coordinator_wake_state;
    let teams = crate::config::teams::discover_teams();

    log_restore_inventory(
        teams.len(),
        settings_snapshot.project_paths.len(),
        setting_on,
        start.persisted.as_ref().map(Vec::len).unwrap_or(0),
    );

    // §224 A.2.5 — RAII guard inside the closure clears the flag
    // on normal exit AND on panic unwind so the daemon can't get
    // stuck advertising "still restoring" forever.
    //
    // §224 G-IMPL-1 — the upper hoisted block already set the flag
    // TRUE before mailbox_poller.start(); we only need to grab a
    // fresh Arc clone here for the RAII guard inside the spawned task.
    let restore_flag_for_task = app.state::<Arc<RestoreInProgress>>().inner().clone();
    let restore_transaction_for_task = start.restore_transaction.clone();

    start_native_restore_startup(
        app_handle,
        start.restore_barrier,
        Arc::new(start.restore_observer_barrier),
        restore_transaction_for_task,
        restore_flag_for_task,
        session_mgr_clone,
        pty_mgr_clone,
        settings_state_clone,
        settings_snapshot,
        start.persisted,
        teams,
        setting_on,
        start.idle_detector,
        start.git_watcher,
        start.discovery_branch_watcher,
        start.resource_monitor,
        start.selection_coordinator,
        start.loop_scheduler,
        start.non_stop_state,
        start.ui_automation_state,
        start.shutdown,
        start.messages,
    );
}

// #1398 - registered here, at the top of setup, and NOT in the
// post-restore tail where a308271c parked it by adjacency: the
// registration depends only on the global-shortcut plugin plus the
// `SettingsState` and `ScreenshotHotkeyState` managed above, so
// waiting for the restore left the hotkey dead for the whole
// restore window, which grows with the number of sessions. Its own
// short task keeps the async settings read off the main thread; a
// `block_on` here would reintroduce the #1341 WebView2 starvation.
fn spawn_screenshot_hotkey_registration(app: &tauri::App) {
    let app_for_hotkey = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let configured = app_for_hotkey
            .state::<SettingsState>()
            .read()
            .await
            .screenshot_capture_hotkey
            .clone();
        match crate::screenshot::register_configured_hotkey(&app_for_hotkey, &configured) {
            Ok(()) => log::info!("[screenshot] global hotkey registered '{}'", configured),
            Err(error) => {
                log::warn!("[screenshot] global hotkey registration failed: {}", error)
            }
        }
    });
}

fn start_git_watchers(
    app: &tauri::App,
    git_rows: Arc<tokio::sync::RwLock<SessionManager>>,
    rows: Arc<tokio::sync::RwLock<SessionManager>>,
    sweep_rows: Arc<tokio::sync::RwLock<SessionManager>>,
    git: &Arc<crate::pty::git_watcher::GitState>,
    shutdown_for_setup: &ShutdownSignal,
) -> (Arc<GitWatcher>, Arc<DiscoveryBranchWatcher>) {
    use crate::pty::git_watcher::GitSweeper;
    // Git branch watcher: polls git branch for each session every 5s
    let settings = app.state::<SettingsState>().inner().clone();
    let ui = app.handle().clone();
    let git_watcher = GitWatcher::with_state(git_rows, ui.clone(), git.clone());
    // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>` (e.g. `update_team`). Must
    // happen BEFORE the `PtyManager::new(..., git_watcher, ...)` move below.
    app.manage(Arc::clone(&git_watcher));
    // Discovery branch watcher: polls git branch for discovered replicas every 15s
    let discovery_branch_watcher = DiscoveryBranchWatcher::with_state(ui, rows, git.clone());
    app.manage(Arc::clone(&discovery_branch_watcher));
    // #1298 - the single global POLLING producer of per-repo git state. Both watchers read its
    // published snapshot instead of spawning `git` themselves.
    //
    // Started HERE and not inside `restore_observer_barrier` for exactly one reason: that barrier
    // gates observers which mutate session metadata or persistence, and this one mutates neither,
    // it only publishes into process-local maps. The construction point buys no head start by
    // itself, because at this point there is nothing to sweep (sessions are restored below,
    // discovery is frontend-driven). Cold start is governed by the empty-round floor instead (plan
    // D3). Do not move this under the barrier, and do not "restore" a head-start rationale that was
    // never true.
    let git_sweeper = GitSweeper::with_state(sweep_rows, settings, git.clone());
    git_sweeper.start(shutdown_for_setup.clone());
    (git_watcher, discovery_branch_watcher)
}

fn start_remote_activity(
    app: &tauri::App,
    git_rows: Arc<tokio::sync::RwLock<SessionManager>>,
    git: Arc<crate::pty::git_watcher::GitState>,
    remote: Arc<crate::pty::remote_watcher::RemoteActivityState>,
    shutdown_for_setup: &ShutdownSignal,
    messages: &Arc<crate::config::injected_messages::MessageWarnings>,
) {
    use crate::pty::remote_watcher::RemoteSweeper;
    let settings = app.state::<SettingsState>().inner().clone();
    // #2064 - the remote activity producer: asks GitHub, through `gh`, whether CI runs on each room
    // repo's exact HEAD and whether the default branch is ahead. Beside `GitSweeper` for the same
    // reason it sits there: it mutates no session metadata, it only publishes into process-local
    // maps. Phase B consumes the transition stream: the notifier started below is its only
    // consumer.
    let (remote_sweeper, remote_transitions) = RemoteSweeper::with_state(
        git_rows,
        settings,
        Box::new({
            let app_for_remote_activity = app.handle().clone();
            move |payload: &crate::pty::remote_watcher::RemoteActivityPayload| {
                let _ = app_for_remote_activity.emit("ac_remote_activity_updated", payload);
            }
        }),
        crate::remote_sweeper_seams(),
        git,
        remote,
    );
    let _ = remote_sweeper.start(shutdown_for_setup.clone());
    // #2064 Phase B - the transition stream's consumer. Started beside the sweeper that produces
    // it, in this same setup closure, and handed a child of the same shutdown token, so the
    // notifier stops when the app does. It injects only into a room orchestrator, and only when
    // both its feature dial and its notify dial are on. The JoinHandle is dropped on purpose: the
    // monitor's lifecycle is its shutdown token and the sweeper's channel, not a join at exit.
    drop(crate::start_remote_alerts(
        app.handle().clone(),
        remote_transitions,
        shutdown_for_setup.token().child_token(),
        messages.clone(),
    ));
}

// #1327 - startup coding-agent update flow. Spawned BEFORE the
// restore task is submitted so every session open (restore, GUI,
// web, phone) blocks inside `create_session_inner_impl` until the
// updates finish or time out.
fn spawn_startup_agent_updates(
    app: &tauri::App,
    agent_update_gate_for_setup: &Arc<agent_update::AgentUpdateGate>,
) {
    let app_for_agent_updates = app.handle().clone();
    let gate_for_setup = Arc::clone(agent_update_gate_for_setup);
    tauri::async_runtime::spawn(async move {
        crate::agent_update::run_startup_updates(app_for_agent_updates, gate_for_setup).await;
    });
}

fn admit_restore_first(
    app: &tauri::App,
    selection_coordinator_for_setup: &crate::session::selection::SelectionCoordinator,
) -> Result<
    (
        crate::session::selection::RestoreBarrierGuard,
        RestoreObserverStartBarrier,
        crate::session::selection::SelectionTransaction<tauri::Wry>,
    ),
    Box<dyn std::error::Error>,
> {
    selection_coordinator_for_setup
        .start(app.handle().clone())
        .map_err(|error| error.to_string())?;
    let restore_observer_barrier = RestoreObserverStartBarrier::default();
    let restore_barrier =
        tauri::async_runtime::block_on(selection_coordinator_for_setup.submit_restore_first())?;
    restore_observer_barrier.mark_restore_admitted()?;
    let restore_transaction = restore_barrier.transaction(app.handle().clone());
    app.state::<Arc<RestoreInProgress>>()
        .0
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok((
        restore_barrier,
        restore_observer_barrier,
        restore_transaction,
    ))
}

fn start_session_observers(
    app: &tauri::App,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    session_mgr_for_scraper: Arc<tokio::sync::RwLock<SessionManager>>,
    idle_detector_for_setup: &Arc<IdleDetector>,
    shutdown_for_setup: &ShutdownSignal,
    messages: &Arc<crate::config::injected_messages::MessageWarnings>,
) {
    // #1056 context-alert actor. Start it before the scraper so the bounded sender
    // exists before the first sample; manage it for final joined shutdown.
    let context_alert_monitor = crate::session::context_alerts::ContextAlertMonitor::start(
        app.handle().clone(),
        shutdown_for_setup.token().child_token(),
        messages.clone(),
    );
    app.manage(Arc::clone(&context_alert_monitor));
    // #1032 context scrape. Must be after `.manage(settings)` above, since the
    // pattern adapter reads settings back out of managed state. Mirrors GitWatcher.
    let context_scraper = ContextScraper::new(
        Arc::new(ScraperRows {
            pty_mgr: pty_mgr.clone(),
            poison_logged: AtomicBool::new(false),
        }),
        Arc::new(ScraperPatterns {
            settings: app.state::<SettingsState>().inner().clone(),
        }),
        Arc::new(ScraperSink {
            app_handle: app.handle().clone(),
        }),
        Arc::new(ScraperSamples {
            sender: context_alert_monitor.sender(),
            closed_logged: AtomicBool::new(false),
            saturated: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }),
        Arc::new(ScraperPersist {
            session_mgr: session_mgr_for_scraper,
        }),
    );
    context_scraper.start(shutdown_for_setup.clone());
    app.manage(Arc::clone(&context_scraper));
    // #2482 agent quota engine. A sibling of the scraper, same construction shape, and
    // like it must come after `.manage(settings)`.
    let agent_quota_engine = AgentQuotaEngine::new(
        Arc::new(QuotaRows {
            pty_mgr: pty_mgr.clone(),
            poison_logged: AtomicBool::new(false),
        }),
        Arc::new(QuotaSources {
            settings: app.state::<SettingsState>().inner().clone(),
        }),
        Arc::new(QuotaSink {
            app_handle: app.handle().clone(),
        }),
    );
    agent_quota_engine.start(shutdown_for_setup.clone());
    app.manage(Arc::clone(&agent_quota_engine));
    start_watcher_engine(app, pty_mgr, idle_detector_for_setup, shutdown_for_setup);
}

fn start_watcher_engine(
    app: &tauri::App,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    idle_detector_for_setup: &Arc<IdleDetector>,
    shutdown_for_setup: &ShutdownSignal,
) {
    // #1171 watcher engine. A SIBLING of the scraper above, not an extension of it:
    // different interval, different modes, its own history. Same construction shape,
    // and for the same reason it must come after `.manage(settings)`.
    let watcher_history: crate::pty::watchers::history::WatcherHistoryState =
        Arc::new(crate::pty::watchers::history::WatcherHistory::default());
    // #2879 - plus the internal Codex activity factory over the idle detector.
    let watcher_engine = WatcherEngine::new_with_activity_factory(
        Arc::new(WatcherBackends {
            pty_mgr: pty_mgr.clone(),
            poison_logged: AtomicBool::new(false),
        }),
        Arc::new(WatcherPatterns {
            settings: app.state::<SettingsState>().inner().clone(),
            log: Default::default(),
        }),
        Arc::new(WatcherSink {
            history: Arc::clone(&watcher_history),
            delivery: WatcherDelivery::to_watchers_window(app.handle().clone()),
        }),
        Arc::clone(&watcher_history),
        Some(codex_activity_factory(idle_detector_for_setup)),
    );
    watcher_engine.start(shutdown_for_setup.clone());
    app.manage(Arc::clone(&watcher_engine));
    app.manage(watcher_history);
}

fn start_menu_guard_and_ipc_observer(
    app: &tauri::App,
    response_close_for_setup: &crate::pty::manager::ResponseCloseState,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    idle_detector_for_setup: &Arc<IdleDetector>,
    ipc_observer_for_setup: &Arc<crate::loops::ipc_observer::IpcObserver>,
    shutdown_for_setup: &ShutdownSignal,
) {
    // #1646 / #1647 - proactive detection of terminal blocking menus
    let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::with_response_close(
        config::settings::BlockingMenusStore::load_from_config_dir(),
        response_close_for_setup.clone(),
    ));
    if app
        .try_state::<crate::pty::input_activity::TypingHoldState>()
        .is_some()
        && app
            .try_state::<crate::pty::manager::ResponseCloseState>()
            .is_some_and(|bound| bound.same_state(response_close_for_setup))
    {
        response_close_for_setup.bind_publisher(crate::pty::manager::ResponseClosePublisher::Hold);
    }
    let manager = tauri::async_runtime::block_on(async {
        let managed = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = managed.read().await;
        guard.clone()
    });
    enable_response_close_readiness(
        response_close_for_setup,
        &manager,
        pty_mgr,
        idle_detector_for_setup,
        &menu_guard,
    );
    crate::config::settings::refresh_shipped_agent_help_from_config_dir();
    menu_guard.start(app.handle().clone(), shutdown_for_setup.clone());
    // (#1652) Started at the top of setup and NOT in the post-restore
    // tail: a freeze detector that waits for the restore is blind for
    // exactly the window that grows with the session count. It depends
    // only on the app handle and the shutdown signal, both available
    // here. Same reasoning as the #1398 hotkey registration above.
    crate::loops::ipc_observer::start(
        app.handle().clone(),
        std::sync::Arc::clone(ipc_observer_for_setup),
        shutdown_for_setup.clone(),
    );
    app.manage(Arc::clone(&menu_guard));
}

fn register_web_command_dispatcher(
    app: &tauri::App,
    session_mgr_for_web: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings_for_web: &SettingsState,
    broadcaster_for_web: &WsBroadcaster,
) -> Result<(), Box<dyn std::error::Error>> {
    let dispatcher =
        crate::web::commands::WebCommandsDispatcher::new(crate::web::commands::WsState::new(
            Arc::clone(session_mgr_for_web),
            Arc::clone(pty_mgr),
            Arc::clone(settings_for_web),
            broadcaster_for_web.clone(),
            app.handle().clone(),
        ));
    if !app.manage(crate::web::command_port::ManagedWebCommandDispatcher(
        Arc::new(dispatcher),
    )) {
        return Err("Web command dispatcher is already registered".into());
    }
    Ok(())
}

fn provision_startup_messages(messages: &crate::config::injected_messages::MessageWarnings) {
    use crate::config::injected_messages as injected;
    if let Err(e) = crate::config::root_agent::ensure_root_agent_dir() {
        log::error!(
            "[root-agent] Failed to provision root agent directory: {}",
            e
        );
    }

    // #1157 - seed and reconcile the operator-editable injected-message templates. Best effort,
    // never fatal, matching the block above.
    //
    // The ordering is NOT load-bearing: `render` resolves from the registry or the embedded default
    // and never fails, so an alert firing before this point still renders correctly. The cost is
    // bounded and small (one read plus at most three small atomic writes on the setup thread), not
    // "non-blocking"; moving it to tokio::spawn would buy nothing and would race the registry load.
    if let Some(dir) = crate::config::config_dir() {
        if let Err(e) = injected::ensure_injected_messages(&dir, messages) {
            log::warn!("[injected-messages] provisioning failed: {}", e);
        }
    }
    // Force the registry once, so the first alert - whose line() runs
    // on a tokio worker - never performs a blocking read there.
    let _ = injected::registry(messages);
}

fn load_normalized_startup_sessions(
    restore_settings_snapshot: &config::settings::AppSettings,
) -> Result<Vec<sessions_persistence::PersistedSession>, String> {
    // #698 — the orphan purge now takes the async `sessions_save_lock()`
    // across its load+filter+save so it cannot clobber a concurrently
    // persisted raise-hand. This sync `setup` body runs on the main
    // thread (outside any runtime worker), so `block_on` is safe here,
    // matching the existing `tauri::async_runtime::block_on` uses in the
    // run-event handler. The lock is uncontended at this point (the
    // mailbox poller and other writers start below), so it returns
    // immediately.
    let mut persisted = load_startup_sessions(restore_settings_snapshot, |paths| {
        tauri::async_runtime::block_on(
            sessions_persistence::load_sessions_purging_outside_project_paths(
                paths,
                restore_settings_snapshot,
            ),
        )
    });
    if let Err(error) = &persisted {
        log::error!(
            "[restore] Project state unavailable; session load/purge skipped: {}",
            error
        );
    }
    if let Ok(persisted) = &mut persisted {
        match normalize_persisted_active_flags(persisted) {
            PersistedActiveFlagNormalization::Zero => {
                log::debug!("[restore] persisted selection flags normalized: zero");
            }
            PersistedActiveFlagNormalization::One { index } => {
                log::debug!(
                    "[restore] persisted selection flags normalized: exactly one \
                        rowIndex={}",
                    index
                );
            }
            PersistedActiveFlagNormalization::Multiple { identities } => {
                log::warn!(
                    "[restore] inconsistent was_active flags count={} rows=[{}]; exact \
                        target cleared for eligible-live fallback",
                    identities.len(),
                    identities.join(", ")
                );
            }
        }
    }
    persisted
}

fn prepare_startup_pty(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    response_close_for_setup: &crate::pty::manager::ResponseCloseState,
) {
    pty_mgr
        .lock()
        .unwrap()
        .bind_response_close(response_close_for_setup.clone());
    install_container_route_remover(pty_mgr);
    pty_mgr
        .lock()
        .unwrap()
        .cleanup_container_orphans_on_startup();
}

fn log_restore_inventory(
    team_count: usize,
    project_path_count: usize,
    setting_on: bool,
    persisted_count: usize,
) {
    // #248 Grinch Z10 — diagnostic: empty `teams` after a project-path rename
    // is a real failure mode; without this line the user sees coords stay
    // dormant with no log clue. Emits exactly once per launch.
    log::info!(
        "[restore] {} teams discovered across {} project paths; setting_on={}; \
            evaluating {} persisted sessions",
        team_count,
        project_path_count,
        setting_on,
        persisted_count
    );
}

fn enable_response_close_readiness(
    response_close_for_setup: &crate::pty::manager::ResponseCloseState,
    manager: &SessionManager,
    pty_mgr: &Mutex<PtyManager>,
    idle_detector_for_setup: &IdleDetector,
    menu_guard: &crate::pty::menu_guard::MenuGuard,
) {
    let response_close_wired = {
        let pty = pty_mgr.lock().unwrap();
        response_close_wiring_matches(
            response_close_for_setup,
            manager,
            &pty,
            idle_detector_for_setup,
            menu_guard,
        )
    };
    if !response_close_wired || response_close_for_setup.try_enable_readiness().is_err() {
        response_close_for_setup.fail_closed();
        log::warn!(
            "[response-close] safety publisher wiring unavailable; readiness remains disabled"
        );
    }
}

fn start_detached_startup_downloads(
    app: &tauri::App,
    update_check_state_for_setup: &UpdateCheckState,
) {
    // Issue #609 - detached "npm update available" check. Fully fail-silent;
    // detached so startup is never blocked or delayed (acceptance criterion).
    {
        let app_handle_for_update = app.handle().clone();
        let update_cache = Arc::clone(update_check_state_for_setup);
        tauri::async_runtime::spawn(async move {
            crate::update_check::run_startup_check(app_handle_for_update, update_cache).await;
        });
    }

    // #1925 - detached remote blocking-menu patterns download. Fail-silent; applies at the
    // next start.
    {
        let app_handle_for_remote_menus = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            crate::update_check::run_remote_blocking_menus_startup(app_handle_for_remote_menus)
                .await;
        });
    }
    // #2133 - detached remote per-agent help download. Fail-silent; applies at the next
    // start.
    {
        let app_handle_for_remote_agent_help = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            crate::update_check::run_remote_agent_help_startup(app_handle_for_remote_agent_help)
                .await;
        });
    }
}
