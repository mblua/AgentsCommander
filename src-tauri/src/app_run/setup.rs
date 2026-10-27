//! The Tauri `.setup` hook of `run()`: everything that needs the live `App`.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{Emitter, Manager};

use crate::{
    agent_update, api, apply_main_display_state, centered_default_main_geometry,
    codex_activity_factory, commands, config, effective_main_display_state,
    install_container_route_remover, is_visible_on_monitors, load_startup_sessions, loops,
    normalize_persisted_active_flags, physical_to_logical, resource_monitor,
    response_close_wiring_matches, sessions_persistence, spawn_co_managed_supervisor,
    start_native_restore_startup, AgentQuotaEngine, ApiServerHandle, ApiServerTask,
    CoManagedSupervisorHandle, CoManagedTrigger, ContextScraper, DiscoveryBranchWatcher,
    GitWatcher, IdleDetector, OutputSenderMap, PersistedActiveFlagNormalization, PtyManager,
    QuotaRows, QuotaSink, QuotaSources, RestoreInProgress, RestoreObserverStartBarrier,
    ScraperPatterns, ScraperPersist, ScraperRows, ScraperSamples, ScraperSink, SessionManager,
    SettingsState, ShutdownSignal, UpdateCheckState, WatcherBackends, WatcherDelivery,
    WatcherEngine, WatcherPatterns, WatcherSink, WebAccessToken, WebServerHandle, WsBroadcaster,
    WEB_SERVER_START_CANCELLED,
};

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
    use crate::config::injected_messages as injected;
    use crate::pty::git_watcher::GitSweeper;
    use crate::pty::remote_watcher::RemoteSweeper;
    use tauri::WebviewUrl;
    let messages = _owners.messages;
    use tauri::WebviewWindowBuilder;
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

    // #1398 - registered here, at the top of setup, and NOT in the
    // post-restore tail where a308271c parked it by adjacency: the
    // registration depends only on the global-shortcut plugin plus the
    // `SettingsState` and `ScreenshotHotkeyState` managed above, so
    // waiting for the restore left the hotkey dead for the whole
    // restore window, which grows with the number of sessions. Its own
    // short task keeps the async settings read off the main thread; a
    // `block_on` here would reintroduce the #1341 WebView2 starvation.
    {
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
    // Git branch watcher: polls git branch for each session every 5s
    let git = _owners.git;
    let settings = app.state::<SettingsState>().inner().clone();
    let ui = app.handle().clone();
    let git_watcher = GitWatcher::with_state(git_rows.clone(), ui.clone(), git.clone());
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
    let git_sweeper = GitSweeper::with_state(sweep_rows, settings.clone(), git.clone());
    git_sweeper.start(shutdown_for_setup.clone());
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
        _owners.remote,
    );
    let _ = remote_sweeper.start(shutdown_for_setup.clone());
    // #2064 Phase B - the transition stream's consumer. Started beside the sweeper that produces
    // it, in this same setup closure, and handed a child of the same shutdown token, so the
    // notifier stops when the app does. It injects only into a room orchestrator, and only when
    // both its feature dial and its notify dial are on. The JoinHandle is dropped on purpose: the
    // monitor's lifecycle is its shutdown token and the sweeper's channel, not a join at exit.
    drop(crate::session::remote_alerts::start(
        app.handle().clone(),
        remote_transitions,
        shutdown_for_setup.token().child_token(),
        messages.clone(),
    ));
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
    pty_mgr
        .lock()
        .unwrap()
        .bind_response_close(response_close_for_setup.clone());
    install_container_route_remover(&pty_mgr);
    pty_mgr
        .lock()
        .unwrap()
        .cleanup_container_orphans_on_startup();
    app.manage(pty_mgr.clone());
    // #1327 - startup coding-agent update flow. Spawned BEFORE the
    // restore task is submitted so every session open (restore, GUI,
    // web, phone) blocks inside `create_session_inner_impl` until the
    // updates finish or time out.
    {
        let app_for_agent_updates = app.handle().clone();
        let gate_for_setup = Arc::clone(&agent_update_gate_for_setup);
        tauri::async_runtime::spawn(async move {
            crate::agent_update::run_startup_updates(app_for_agent_updates, gate_for_setup).await;
        });
    }
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
        Some(codex_activity_factory(&idle_detector_for_setup)),
    );
    watcher_engine.start(shutdown_for_setup.clone());
    app.manage(Arc::clone(&watcher_engine));
    app.manage(watcher_history);
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
            .is_some_and(|bound| bound.same_state(&response_close_for_setup))
    {
        response_close_for_setup.bind_publisher(crate::pty::manager::ResponseClosePublisher::Hold);
    }
    let manager = tauri::async_runtime::block_on(async {
        let managed = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = managed.read().await;
        guard.clone()
    });
    let response_close_wired = {
        let pty = pty_mgr.lock().unwrap();
        response_close_wiring_matches(
            &response_close_for_setup,
            &manager,
            &pty,
            &idle_detector_for_setup,
            &menu_guard,
        )
    };
    if !response_close_wired || response_close_for_setup.try_enable_readiness().is_err() {
        response_close_for_setup.fail_closed();
        log::warn!(
            "[response-close] safety publisher wiring unavailable; readiness remains disabled"
        );
    }
    crate::config::settings::refresh_shipped_agent_help_from_config_dir();
    menu_guard.start(app.handle().clone(), shutdown_for_setup.clone());
    // (#1652) Started at the top of setup and NOT in the post-restore
    // tail: a freeze detector that waits for the restore is blind for
    // exactly the window that grows with the session count. It depends
    // only on the app handle and the shutdown signal, both available
    // here. Same reasoning as the #1398 hotkey registration above.
    crate::loops::ipc_observer::start(
        app.handle().clone(),
        std::sync::Arc::clone(&ipc_observer_for_setup),
        shutdown_for_setup.clone(),
    );
    app.manage(Arc::clone(&menu_guard));
    // The authoritative scope of the activity window. `open_watchers_window` writes it
    // before every emit and the window pulls it after subscribing, so a re-scope that
    // races the window's own load is recovered instead of dropped in silence.
    app.manage(commands::window::WatchersScopeState::default());
    let dispatcher =
        crate::web::commands::WebCommandsDispatcher::new(crate::web::commands::WsState {
            session_mgr: Arc::clone(&session_mgr_for_web),
            pty_mgr: Arc::clone(&pty_mgr),
            settings: Arc::clone(&settings_for_web),
            broadcaster: broadcaster_for_web.clone(),
            app_handle: app.handle().clone(),
        });
    if !app.manage(crate::web::command_port::ManagedWebCommandDispatcher(
        Arc::new(dispatcher),
    )) {
        return Err("Web command dispatcher is already registered".into());
    }
    // Start web server if enabled in settings
    {
        let web_settings = config::settings::load_settings();
        if web_settings.web_server_enabled {
            let ws_handle = app.state::<WebServerHandle>().inner().clone();
            let waiter = commands::config::begin_web_server_start(
                ws_handle.clone(),
                settings_for_web,
                web_token_for_server,
                session_mgr_for_web,
                pty_mgr.clone(),
                broadcaster_for_web,
                app.handle().clone(),
                shutdown_for_setup.clone(),
            );
            match tauri::async_runtime::block_on(waiter.wait()) {
                Ok(true) => {
                    if let Some((bind, port)) = ws_handle.snapshot().endpoint {
                        println!(
                            "[web-token] Remote URL: \
                                http://{}:{}/?window=main&remoteToken={}",
                            bind,
                            port,
                            web_access_token.value()
                        );
                    }
                }
                Ok(false) => {}
                Err(error) if error == WEB_SERVER_START_CANCELLED => {
                    log::info!("[web-server] autostart cancelled by lifecycle stop");
                }
                Err(error) => {
                    log::warn!("[web-server] startup failed: {}", error);
                }
            }
        }
    }

    // #791 - start the control-plane API server if enabled in settings.
    // Opt-in (default false), mirroring the web server block above. The
    // managed handle is stored only after bind readiness is confirmed.
    {
        let api_settings = config::settings::load_settings();
        if api_settings.api_server_enabled {
            let bind = api_settings.api_server_bind.clone();
            let port = api_settings.api_server_port;
            let api_shutdown = shutdown_for_setup.token().child_token();
            let server_start = api::start_server(
                bind,
                port,
                app.handle().clone(),
                session_mgr_for_api.clone(),
                pty_mgr.clone(),
                api_shutdown.clone(),
            );
            match tauri::async_runtime::block_on(api::wait_for_startup_ready(
                server_start.readiness,
            )) {
                Ok(bound_addr) => {
                    let api_handle = app.state::<ApiServerHandle>();
                    if let Err(e) = api_handle.store_if_idle(ApiServerTask::new(
                        server_start.join_handle,
                        api_shutdown,
                        bound_addr,
                    )) {
                        log::error!("[api-server] failed to store server handle: {}", e);
                    }
                }
                Err(err) => {
                    api_shutdown.cancel();
                    log::warn!("[api-server] startup failed: {}", err);
                }
            }
        }
    }

    // Issue #609 - detached "npm update available" check. Fully fail-silent;
    // detached so startup is never blocked or delayed (acceptance criterion).
    {
        let app_handle_for_update = app.handle().clone();
        let update_cache = Arc::clone(&update_check_state_for_setup);
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
        if let Err(e) = injected::ensure_injected_messages(&dir, messages.as_ref()) {
            log::warn!("[injected-messages] provisioning failed: {}", e);
        }
    }
    // Force the registry once, so the first alert - whose line() runs
    // on a tokio worker - never performs a blocking read there.
    let _ = injected::registry(messages.as_ref());

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
    // #698 — the orphan purge now takes the async `sessions_save_lock()`
    // across its load+filter+save so it cannot clobber a concurrently
    // persisted raise-hand. This sync `setup` body runs on the main
    // thread (outside any runtime worker), so `block_on` is safe here,
    // matching the existing `tauri::async_runtime::block_on` uses in the
    // run-event handler. The lock is uncontended at this point (the
    // mailbox poller and other writers start below), so it returns
    // immediately.
    let mut persisted = load_startup_sessions(&restore_settings_snapshot, |paths| {
        tauri::async_runtime::block_on(
            sessions_persistence::load_sessions_purging_outside_project_paths(
                paths,
                &restore_settings_snapshot,
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
    let restore_flag = app.state::<Arc<RestoreInProgress>>().inner().clone();
    restore_flag
        .0
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    // Load saved window geometry
    let saved_settings = config::settings::load_settings();

    // Collect available monitor bounds (physical) + scale factor for geometry validation
    // Tuple: (x, y, x2, y2, scale_factor) — all positions/sizes in physical pixels
    let monitors: Vec<(f64, f64, f64, f64, f64)> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let pos = m.position();
            let size = m.size();
            (
                pos.x as f64,
                pos.y as f64,
                pos.x as f64 + size.width as f64,
                pos.y as f64 + size.height as f64,
                m.scale_factor(),
            )
        })
        .collect();

    log::info!("[window-setup] {} monitors detected", monitors.len());
    for (i, (mx, my, mx2, my2, scale)) in monitors.iter().enumerate() {
        log::info!(
            "[window-setup]   monitor {}: ({}, {}) -> ({}, {}) scale={}",
            i,
            mx,
            my,
            mx2,
            my2,
            scale
        );
    }

    // Determine primary monitor size for the default "centered main" layout.
    // Convert to logical pixels (physical / scale) since WebviewWindowBuilder
    // ::inner_size() and ::position() expect logical coordinates.
    let primary = app.primary_monitor().ok().flatten();
    let primary_scale = primary.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let (screen_w, screen_h) = primary
        .as_ref()
        .map(|m| {
            let s = m.size();
            (
                s.width as f64 / primary_scale,
                s.height as f64 / primary_scale,
            )
        })
        .unwrap_or((1920.0, 1080.0));
    let primary_x = primary
        .as_ref()
        .map(|m| m.position().x as f64 / primary_scale)
        .unwrap_or(0.0);
    let primary_y = primary
        .as_ref()
        .map(|m| m.position().y as f64 / primary_scale)
        .unwrap_or(0.0);

    // Default main window: centered at 1400×900, or the primary monitor size
    // minus a small margin if the screen is narrower than 1400.
    let default_main = centered_default_main_geometry(primary_x, primary_y, screen_w, screen_h);

    fn log_main_window_info(win: &tauri::WebviewWindow) {
        let pid = std::process::id();
        let pos = win.outer_position().ok();
        let size = win.outer_size().ok();
        let maximized = win.is_maximized().ok();
        log::info!(
            "[test-window] actual pid={} position={:?} size={:?} maximized={:?}",
            pid,
            pos,
            size,
            maximized
        );
        println!(
            concat!(
                "{{\"event\":\"testWindowInfo\",\"pid\":{},\"position\":{},",
                "\"size\":{},\"maximized\":{}}}"
            ),
            pid,
            serde_json::to_string(&pos.map(|p| serde_json::json!({ "x": p.x, "y": p.y })))
                .unwrap_or_else(|_| "null".to_string()),
            serde_json::to_string(
                &size.map(|s| serde_json::json!({ "width": s.width, "height": s.height }))
            )
            .unwrap_or_else(|_| "null".to_string()),
            serde_json::to_string(&maximized).unwrap_or_else(|_| "null".to_string())
        );
    }

    fn apply_test_window_placement(
        win: &tauri::WebviewWindow,
        geo: &crate::testability::window_placement::TestWindowPlacement,
    ) -> bool {
        let x = geo.x.round() as i32;
        let y = geo.y.round() as i32;
        let width = geo.width.round().max(1.0) as u32;
        let height = geo.height.round().max(1.0) as u32;

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::{POINT, RECT};
            use windows_sys::Win32::Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowPlacement, IsZoomed, SetWindowPlacement, SetWindowPos, ShowWindow,
                SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOWMAXIMIZED,
                WINDOWPLACEMENT,
            };

            match win.hwnd() {
                Ok(hwnd) => unsafe {
                    let requested = RECT {
                        left: x,
                        top: y,
                        right: x.saturating_add(width as i32),
                        bottom: y.saturating_add(height as i32),
                    };
                    let monitor = MonitorFromRect(&requested, MONITOR_DEFAULTTONEAREST);
                    if monitor.is_null() {
                        log::warn!(
                            "[test-window] MonitorFromRect returned null for requested \
                                rect ({}, {}) {}x{}",
                            x,
                            y,
                            width,
                            height
                        );
                    } else {
                        let mut monitor_info = MONITORINFO {
                            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                            rcMonitor: RECT {
                                left: 0,
                                top: 0,
                                right: 0,
                                bottom: 0,
                            },
                            rcWork: RECT {
                                left: 0,
                                top: 0,
                                right: 0,
                                bottom: 0,
                            },
                            dwFlags: 0,
                        };
                        if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
                            log::warn!(
                                "[test-window] GetMonitorInfoW failed for requested \
                                    rect ({}, {}) {}x{}",
                                x,
                                y,
                                width,
                                height
                            );
                        } else {
                            log::info!(
                                "[test-window] selected monitor rect=({}, {}) {}x{} \
                                    work=({}, {}) {}x{} for requested rect ({}, {}) \
                                    {}x{} maximized={}",
                                monitor_info.rcMonitor.left,
                                monitor_info.rcMonitor.top,
                                monitor_info.rcMonitor.right - monitor_info.rcMonitor.left,
                                monitor_info.rcMonitor.bottom - monitor_info.rcMonitor.top,
                                monitor_info.rcWork.left,
                                monitor_info.rcWork.top,
                                monitor_info.rcWork.right - monitor_info.rcWork.left,
                                monitor_info.rcWork.bottom - monitor_info.rcWork.top,
                                x,
                                y,
                                width,
                                height,
                                geo.maximized
                            );
                        }
                    }

                    if IsZoomed(hwnd.0 as _) != 0 || geo.maximized {
                        ShowWindow(hwnd.0 as _, SW_RESTORE);
                    }
                    let ok = SetWindowPos(
                        hwnd.0 as _,
                        std::ptr::null_mut(),
                        x,
                        y,
                        width as i32,
                        height as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    if ok == 0 {
                        log::warn!("[test-window] native SetWindowPos failed");
                    }
                    if geo.maximized {
                        let mut placement = WINDOWPLACEMENT {
                            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                            flags: 0,
                            showCmd: SW_SHOWMAXIMIZED as u32,
                            ptMinPosition: POINT { x: -1, y: -1 },
                            ptMaxPosition: POINT { x: -1, y: -1 },
                            rcNormalPosition: requested,
                        };
                        if GetWindowPlacement(hwnd.0 as _, &mut placement) == 0 {
                            log::warn!("[test-window] GetWindowPlacement failed before maximize");
                        }
                        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                        placement.showCmd = SW_SHOWMAXIMIZED as u32;
                        placement.rcNormalPosition = requested;
                        if SetWindowPlacement(hwnd.0 as _, &placement) == 0 {
                            log::warn!("[test-window] SetWindowPlacement maximize failed");
                            ShowWindow(hwnd.0 as _, SW_SHOWMAXIMIZED);
                        }
                    }
                    return true;
                },
                Err(e) => {
                    log::warn!("[test-window] failed to get HWND: {}", e);
                }
            }
        }

        if let Err(e) = win.set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height })) {
            log::warn!("[test-window] failed to set physical size: {}", e);
        }
        if let Err(e) =
            win.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
        {
            log::warn!("[test-window] failed to set physical position: {}", e);
        }
        false
    }

    // Resolve main geometry: saved (physical) -> validate -> convert to logical ->
    // fallback.
    // First-boot-after-upgrade users will have `main_geometry` seeded from legacy
    // `terminal_geometry` via the migration in `config::settings::load_settings`.
    let main_geo = if let Some(test_geo) = &test_window_placement {
        let requested = config::settings::WindowGeometry {
            x: test_geo.x,
            y: test_geo.y,
            width: test_geo.width,
            height: test_geo.height,
        };
        let logical = physical_to_logical(&requested, &monitors);
        log::info!(
            "[test-window] requested physical ({}, {}) {}x{} maximized={} -> logical \
                ({}, {}) {}x{}",
            requested.x,
            requested.y,
            requested.width,
            requested.height,
            test_geo.maximized,
            logical.x,
            logical.y,
            logical.width,
            logical.height
        );
        logical
    } else {
        match &saved_settings.main_geometry {
            Some(geo) if is_visible_on_monitors(geo, &monitors) => {
                let logical = physical_to_logical(geo, &monitors);
                log::info!(
                    "[window-setup] main: saved physical ({}, {}) {}x{} -> logical ({}, \
                        {}) {}x{}",
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height,
                    logical.x,
                    logical.y,
                    logical.width,
                    logical.height
                );
                logical
            }
            Some(geo) => {
                log::warn!(
                    "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, \
                        falling back to centered default",
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height
                );
                default_main.clone()
            }
            None => {
                log::info!("[window-setup] main: no saved geometry, using centered default");
                default_main.clone()
            }
        }
    };

    // #2348: resolve the startup display state once. A test placement's
    // `maximized` boolean overrides the saved state entirely; otherwise the
    // saved state applies (and a saved maximize is only a request, never a
    // fatal failure: `apply_main_display_state` logs and keeps the window
    // usable).
    let effective_display_state = effective_main_display_state(
        saved_settings.main_window_display_state,
        test_window_placement.as_ref(),
    );

    // Create the unified Main window (replaces sidebar + terminal windows).
    let main_win = WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::App("index.html?window=main".into()),
    )
    .title(config::profile::app_title())
    .icon(icon)
    .expect("Failed to set main window icon")
    .min_inner_size(800.0, 500.0)
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .inner_size(main_geo.width, main_geo.height)
    .position(main_geo.x, main_geo.y)
    .build()?;

    if let Some(test_geo) = &test_window_placement {
        let native_handled = apply_test_window_placement(&main_win, test_geo);
        if effective_display_state == config::settings::MainWindowDisplayState::Maximized
            && !native_handled
        {
            if let Err(e) = main_win.maximize() {
                log::warn!("[test-window] failed to maximize main window: {}", e);
            }
        }
        log_main_window_info(&main_win);
    } else {
        apply_main_display_state(effective_display_state, || main_win.maximize());
    }

    if saved_settings.main_always_on_top {
        let _ = main_win.set_always_on_top(true);
    }

    // Suppress unused variable warning
    let _ = &main_win;

    // Restore sessions from last run
    //
    // §224 G-IMPL-1 — `persisted` and `restore_flag` are hoisted above
    // mailbox_poller.start() (see comment block there). `persisted` is
    // reused here; the flag is already TRUE when we enter this block.
    // #1341 - the restore now runs as a spawned runtime task
    // (spawn_restore_startup), never inside a main-thread block_on; the
    // main thread returns to the event loop so the webview can load
    // while the #1327 update gate is pending.
    {
        use tauri::Manager;
        let session_mgr_clone = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
            .clone();
        let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let settings_state_clone = app.state::<SettingsState>().inner().clone();
        let app_handle = app.handle().clone();

        // #248 — read the new setting and always discover teams (the coord check
        // is run for every persisted session, regardless of the setting's value).
        let settings_snapshot = restore_settings_snapshot.clone();
        let setting_on = settings_snapshot.restore_coordinator_wake_state;
        let teams = crate::config::teams::discover_teams();

        // #248 Grinch Z10 — diagnostic: empty `teams` after a project-path rename
        // is a real failure mode; without this line the user sees coords stay
        // dormant with no log clue. Emits exactly once per launch.
        log::info!(
            "[restore] {} teams discovered across {} project paths; setting_on={}; \
                evaluating {} persisted sessions",
            teams.len(),
            settings_snapshot.project_paths.len(),
            setting_on,
            persisted.as_ref().map(Vec::len).unwrap_or(0)
        );

        // §224 A.2.5 — RAII guard inside the closure clears the flag
        // on normal exit AND on panic unwind so the daemon can't get
        // stuck advertising "still restoring" forever.
        //
        // §224 G-IMPL-1 — the upper hoisted block already set the flag
        // TRUE before mailbox_poller.start(); we only need to grab a
        // fresh Arc clone here for the RAII guard inside the spawned task.
        let restore_flag_for_task = app.state::<Arc<RestoreInProgress>>().inner().clone();
        let restore_transaction_for_task = restore_transaction.clone();

        start_native_restore_startup(
            app_handle,
            restore_barrier,
            Arc::new(restore_observer_barrier),
            restore_transaction_for_task,
            restore_flag_for_task,
            session_mgr_clone,
            pty_mgr_clone,
            settings_state_clone,
            settings_snapshot,
            persisted,
            teams,
            setting_on,
            idle_detector_for_setup,
            git_watcher,
            discovery_branch_watcher,
            resource_monitor_for_setup,
            selection_coordinator_for_setup,
            loop_scheduler_for_setup,
            non_stop_state_for_setup,
            ui_automation_state_for_setup,
            shutdown_for_setup,
            messages,
        );
    }

    Ok(())
}
