//! The desktop app entry: boot preamble, managed state and the Tauri builder
//! chain. The `.setup` hook lives in `setup.rs` and the run-event handler in
//! `exit.rs`.

pub(crate) mod exit;
mod setup;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::Manager;

use crate::{
    agent_update, capture, commands, config, emit_session_idle_edge, log_naming_migration_summary,
    loops, network, preflight_config_startup, prepare_app_outbox, resource_monitor, screenshot,
    ApiServerHandle, CoManagedSupervisorHandle, ConfigSeedLockState, DetachedSessionsState,
    IdleDetector, MasterToken, OutputSenderMap, PendingSelfClear, QuitGate, RestoreInProgress,
    SessionManager, SettingsState, ShutdownSignal, SpecBoardState, StartupError,
    TelegramBridgeManager, TelegramBridgeState, UpdateCheckState, VoiceTracker, VoiceTrackingState,
    WebAccessToken, WebServerHandle, WsBroadcaster,
};

pub(crate) fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
    owners: crate::AppOwners,
) -> Result<(), StartupError> {
    // #1842: FIRST STATEMENT OF run(), deliberately. GTK's Wayland backend
    // calls wl_display_connect(), which unsetenv()s WAYLAND_SOCKET, so a
    // session identified only by an inherited fd loses its only signal the
    // moment GTK starts. Tao initialises GTK when it builds the event loop, in
    // `.run(..)`; `.setup` and register_configured_hotkey (2809) both run after
    // that and would read an erased variable and answer X11 — the silent dead
    // hotkey this phase exists to prevent.
    //
    // THE REAL INVARIANT IS "before anything initialises GTK", not "before
    // `.setup`". §8's controls check the second because it is mechanically
    // checkable; the two coincide only because nothing in this crate touches
    // gtk/gdk/tao before the event loop, which §8 control 5 pins. If that ever
    // stops being true, this line must move, not the control.
    #[cfg(target_os = "linux")]
    let display_env = crate::screenshot::display_env_snapshot();

    let BootPreamble {
        master_token,
        app_outbox,
        ui_automation_state,
        web_access_token,
    } = prepare_boot(&owners, ui_automation_enabled)?;

    // Create WS broadcaster (shared between Tauri commands and web server)
    let broadcaster = WsBroadcaster::new();

    let response_close = crate::pty::manager::ResponseCloseState::default();
    let response_close_for_setup = response_close.clone();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        SessionManager::with_response_close(response_close.clone(), owners.activity.clone()),
    ));
    let shutdown_signal = ShutdownSignal::new();
    let terminal_snapshot_state =
        crate::pty::terminal_snapshot::TerminalSnapshotState::new(shutdown_signal.clone());
    let selection_coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown_signal.token().clone(),
    );

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));

    // #2232 phase 7: the Co-managed supervisor. Created before the idle
    // detector so its callback can read the armed flag synchronously and hand
    // the edge over without touching settings, disk or a session lookup.
    let (co_managed_supervisor, co_managed_triggers) =
        CoManagedSupervisorHandle::with_capture_state(owners.capture.clone());
    let co_managed_supervisor_for_idle = co_managed_supervisor.clone();
    let co_managed_supervisor_for_setup = co_managed_supervisor.clone();

    // Idle detector: emits session_idle / session_busy events.
    // Callbacks run on native threads (watcher + PTY read loop).
    // AppHandle.emit() is sync and thread-safe, so no tokio needed.
    // AppHandle is set in setup() via OnceLock; callbacks no-op until then.
    let app_handle_lock: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let handle_for_idle = Arc::clone(&app_handle_lock);
    let handle_for_busy = Arc::clone(&app_handle_lock);
    let response_close_for_idle = response_close.clone();
    let response_close_for_busy = response_close.clone();
    let idle_detector = IdleDetector::with_activity_coalescer(
        Arc::clone(&owners.activity),
        move |id| {
            response_close_for_idle.observe_detector(id, false, std::time::Instant::now());
            log::debug!("[idle] >>> EMIT session_idle for {}", &id.to_string()[..8]);
            if let Some(app) = handle_for_idle.get() {
                // #2232 phase 7: the Co-managed decision travels in the same
                // payload, emitted synchronously and FIRST (epic 3.6). A
                // separate later event would always paint waiting before red.
                // `is_armed` is lock-free and never reads settings or disk.
                let _ = emit_session_idle_edge(
                    app,
                    &co_managed_supervisor_for_idle.armed,
                    Some(&co_managed_supervisor_for_idle),
                    id,
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                let app_for_idle = app.clone();
                // #1682 - read the control-write mark on the watcher thread, at
                // the edge instant. `Option<Duration>` is `Copy`, so it moves
                // into the spawned task with no clone and no guard held.
                let control_write_age = app
                    .try_state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                    .and_then(|detector| detector.control_write_age(id));
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_idle(id).await;
                    crate::config::sessions_persistence::persist_current_state_prune_dormant(&mgr)
                        .await;
                    // #1682 - a busy->idle edge on an armed session stamps
                    // `tooling.lastAgentMessageAt`. A no-op unless a message write reached
                    // an arming site for this session in this process (R7 and R8 arm with
                    // nothing submitted), the user has no recent unsubmitted input in it,
                    // and no recent control write of ours is what re-opened the busy period.
                    crate::commands::session::record_agent_turn_completed(
                        &app_for_idle,
                        &mgr,
                        id,
                        control_write_age,
                    )
                    .await;
                    if let Some(scheduler) =
                        app_for_idle.try_state::<Arc<loops::scheduler::LoopScheduler>>()
                    {
                        scheduler
                            .inner()
                            .on_session_idle(app_for_idle.clone(), id)
                            .await;
                    }
                });
            }
        },
        move |id| {
            response_close_for_busy.observe_detector(id, true, std::time::Instant::now());
            log::debug!("[idle] >>> EMIT session_busy for {}", &id.to_string()[..8]);
            if let Some(app) = handle_for_busy.get() {
                let _ = tauri::Emitter::emit(
                    app,
                    "session_busy",
                    serde_json::json!({ "id": id.to_string() }),
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_busy(id).await;
                    crate::config::sessions_persistence::persist_current_state_prune_dormant(&mgr)
                        .await;
                });
            }
        },
    );
    response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::Detector);
    let response_close_for_output = response_close.clone();
    if !idle_detector.bind_raw_output_observer(response_close.wiring_identity(), move |id, now| {
        response_close_for_output.observe_raw_output(id, now)
    }) {
        response_close.fail_closed();
    }
    response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::RawOutput);
    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_git_sweeper = Arc::clone(&session_mgr);
    let session_mgr_for_discovery = Arc::clone(&session_mgr);
    let session_mgr_for_web = Arc::clone(&session_mgr);
    let session_mgr_for_api = Arc::clone(&session_mgr);
    let activity_for_exit = Arc::clone(&owners.activity);
    let session_mgr_for_exit = Arc::clone(&session_mgr);
    // #1088 - handed to `ScraperPersist` so the context scraper can persist
    // changed readings through the same path the idle/busy callbacks use.
    let session_mgr_for_scraper = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);
    // #552 manage the IdleDetector so the shared user-message helper, the mailbox
    // wake path, and the auto-close task can reach its silence clock.
    let idle_detector_for_state = Arc::clone(&idle_detector);
    let idle_detector_for_setup = Arc::clone(&idle_detector);
    let broadcaster_for_pty = broadcaster.clone();
    let broadcaster_for_web = broadcaster.clone();
    let web_token_for_server = Arc::clone(&web_access_token);

    // #2232 phase 3 section 5.1 / phase 4 section 4.2: the **single**
    // `CaptureRegistry` of the app. It is stored in Tauri state below and
    // handed to the bridge manager, so the supervisor can open a session's
    // endpoints and pass the live sender into both watchers. Without this the
    // phase-1 and phase-3 chain is unreachable from production.
    let capture_registry = Arc::new(capture::registry::CaptureRegistry::new());
    let capture_registry_for_state = Arc::clone(&capture_registry);

    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::with_captures(output_senders, capture_registry),
    ));

    let RuntimeState {
        settings,
        coordinator_clocks,
        coordinator_clocks_for_exit,
        resource_monitor_state,
        screenshot_capture_state,
        screenshot_hotkey_state,
        settings_for_web,
        detached_sessions,
        voice_tracking,
        spec_board_state,
        loop_scheduler,
        loop_scheduler_for_setup,
        non_stop_state,
        non_stop_state_for_setup,
        ipc_observer,
        ipc_observer_for_setup,
        ipc_observer_for_handler,
        config_seed_lock,
        update_check_state,
        update_check_state_for_setup,
        agent_update_gate,
        agent_update_gate_for_setup,
        agent_install_cache,
        shutdown_for_setup,
        shutdown_for_exit,
        selection_coordinator_for_setup,
        selection_coordinator_for_exit,
        tg_mgr_for_exit,
        resource_monitor_for_setup,
        resource_monitor_for_exit,
        ui_automation_state_for_setup,
        ui_automation_state_for_exit,
        message_store_state,
        pty_target_gate_state,
    } = prepare_runtime_state(
        &owners,
        &response_close,
        &shutdown_signal,
        &selection_coordinator,
        &tg_mgr,
        &ui_automation_state,
    );

    // #714/#1842/#2079 clipboard + global-shortcut plugins are referenced ONLY
    // on Windows, Linux and macOS; every other target never links them. The
    // rest of the builder chain is shared. This predicate must match
    // `Cargo.toml` and `screenshot/mod.rs`.
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    let builder = builder
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        crate::screenshot::begin_capture_from_hotkey(app.clone());
                    }
                })
                .build(),
        );

    #[cfg(target_os = "linux")]
    let builder = builder.manage(display_env);

    let builder = crate::commands::session::selection_executor::install(builder);

    builder
        .manage(owners.clone())
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(selection_coordinator)
        .manage(tg_mgr)
        .manage(capture_registry_for_state)
        .manage(network::OutboundNetwork::new().expect("failed to build shared network clients"))
        .manage(Arc::clone(&resource_monitor_state))
        .manage(voice_tracking)
        .manage(settings)
        .manage(idle_detector_for_state) // #552 managed type: Arc<IdleDetector>
        .manage(coordinator_clocks) // #552 managed type: CoordinatorClocksState
        .manage(std::sync::Arc::new(
            crate::session::purge_guard::PurgeGuard::default(),
        )) // #885
        .manage(detached_sessions.clone())
        .manage(spec_board_state.clone())
        .manage(loop_scheduler.clone())
        .manage(non_stop_state)
        .manage(ipc_observer)
        .manage(web_access_token.clone())
        .manage(broadcaster.clone())
        .manage(WebServerHandle::default())
        .manage(ApiServerHandle::default())
        .manage(message_store_state)
        .manage(pty_target_gate_state)
        .manage(config_seed_lock)
        .manage(update_check_state)
        .manage(agent_update_gate)
        .manage(agent_install_cache)
        .manage(ui_automation_state)
        .manage(terminal_snapshot_state)
        .manage(shutdown_signal)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(PendingSelfClear::default()))
        .manage(screenshot_capture_state) // #714
        .manage(screenshot_hotkey_state) // #714
        .manage(crate::pty::input_activity::new_state()) // #871 substantive-input tracker
        .manage(response_close.clone())
        .manage(crate::pty::input_activity::new_typing_hold_state()) // #2336 typing hold
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(QuitGate::new())) // #2296 managed type: Arc<QuitGate>
        .setup(move |app| {
            setup::setup(
                app,
                setup::SetupCtx {
                    owners: owners.clone(),
                    test_window_placement,
                    app_handle_lock,
                    co_managed_supervisor_for_setup,
                    co_managed_triggers,
                    response_close_for_setup,
                    session_mgr_for_git,
                    session_mgr_for_git_sweeper,
                    session_mgr_for_discovery,
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
                },
            )
        })
        // (#1652) Every invoke that reaches the app handler is stamped here,
        // before dispatch: the only place in the process that sees the
        // frontend -> backend direction as a stream, which is what lets
        // `[ipc-observer] SILENCE` speak about the CHANNEL and not one command.
        // Entry only - `generate_handler!` returns as soon as the command is
        // dispatched, so there is no exit to hook. The exit side is the
        // renderer's `settled` accounting (phase 2); the pair separates "never
        // arrived" from "never came back".
        //
        // UNSTABLE API: `tauri::ipc::Invoke`'s own doc comment says it "is used
        // internally by macros and is explicitly **NOT** stable"
        // (tauri-2.10.3/src/ipc/mod.rs:209). `Cargo.lock` pins 2.10.3 and this
        // wrapper is the ONLY place in the app depending on an explicitly
        // unstable tauri type. The next tauri bump must re-check three things:
        // `Invoke::message` public, `InvokeMessage::command()` public, and
        // `generate_handler!` still expanding to a bindable closure expression.
        //
        // Blind spot, stated: Tauri routes plugin commands (`plugin:window|*`,
        // `plugin:event|emit`, `plugin:dialog|open`) through `manager.extend_api`
        // in a branch that never reaches `run_invoke_handler`
        // (tauri-2.10.3/src/webview/mod.rs:1840-1890), so they never reach this
        // closure - and the renderer's registry does not see them either, because
        // `@tauri-apps/api`'s window/event/dialog helpers call
        // `__TAURI_INTERNALS__.invoke` directly, not `TauriTransport.invoke`. The
        // two sides therefore stay symmetric and no false "sent but never
        // entered" signal is produced. The `[ipc-blackbox] coverage:` line repeats
        // this in every record block.
        .invoke_handler(crate::invoke_handler(ipc_observer_for_handler))
        .build({
            #[expect(clippy::exit, reason = "Tauri generated thread failure exits")]
            let context = tauri::generate_context!();
            context
        })
        .expect("error while building application")
        .run({
            let detached_set = detached_sessions.clone();
            let spec_board_state = spec_board_state.clone();
            let ctx = exit::ExitCtx {
                detached_set,
                spec_board_state,
                session_mgr_for_exit,
                activity_for_exit,
                coordinator_clocks_for_exit,
                shutdown_for_exit,
                selection_coordinator_for_exit,
                tg_mgr_for_exit,
                resource_monitor_for_exit,
                ui_automation_state_for_exit,
            };
            crate::exit_callback(ctx, commands::spec_board::spec_board_close_all)
        });
    Ok(())
}

struct BootPreamble {
    master_token: MasterToken,
    app_outbox: crate::AppOutbox,
    ui_automation_state: crate::testability::ui_automation::UiAutomationState,
    web_access_token: Arc<WebAccessToken>,
}

fn prepare_boot(
    owners: &crate::AppOwners,
    ui_automation_enabled: bool,
) -> Result<BootPreamble, StartupError> {
    preflight_config_startup(owners.startup_summary.as_ref())?;

    // Same backend the CLI path now installs in `main.rs` — see `logging.rs`
    // for the rationale. Idempotent, so a hypothetical second call (or the
    // CLI path having already run in this process) is a no-op.
    crate::logging::init_logger();
    log_naming_migration_summary(owners.startup_summary.as_ref());

    // Generate master token — printed to stdout and persisted to master-token.txt for CLI use
    let master_token = MasterToken::new(uuid::Uuid::new_v4().to_string());

    // Create instance-private outbox directory and clean up stale ones
    let config_dir = config::config_dir().expect("Cannot determine home directory");
    let instances_dir = config_dir.join(crate::config::instance_artifacts::INSTANCES_DIR_NAME);

    // Clean up old instance dirs (from previous runs)
    if let Ok(entries) = std::fs::read_dir(&instances_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
        log::info!("[app-outbox] Cleaned stale instance directories");
    }

    // #769 Phase 1 + #1318 - seed the externalized coding-agent catalog and the
    // dest-keyed default config-folder masters once per registered project
    // (whole-file seed-once + create-if-absent, fail-soft; never aborts boot).
    // Must run before the frontend can call `get_coding_agent_catalog`. The
    // read-only CLI loader avoids a boot-time settings write (no root_token
    // auto-gen); the Tauri setup's own `load_settings()` performs the standard
    // migrations. The per-project steady-state pre-check keeps the common
    // already-seeded case lock-free.
    let settings = config::settings::load_settings_for_cli();
    let registered_roots = config::coding_agents_catalog::registered_project_roots(&settings);
    if registered_roots.is_empty() {
        // #2021: with no registered project, initialize or refresh the instance
        // catalog at <config_dir>/coding-agents so the Welcome surface lists
        // the built-in agents on first run. Fail-soft: log-only, never aborts
        // boot.
        if let Some(dir) = config::config_dir() {
            config::coding_agents_catalog::ensure_seeded_instance(&dir);
        }
    }
    for root in registered_roots {
        config::coding_agents_catalog::ensure_seeded_for_project(&root);
    }

    let instance_id = uuid::Uuid::new_v4().to_string();
    // #1149 - open the activity run here, before the rest of boot: a panic in the
    // remaining path then still leaves a run that had started and never stopped,
    // which the next startup reports as unclean. This is also the last point at
    // which `daemon.pid` still holds the PREVIOUS writer's PID, which is what
    // lets the scan tell a dead predecessor from a live sibling.
    let activity_log_enabled = config::settings::read_activity_log_enabled_only();
    crate::config::activity_log::init_run(
        &config_dir,
        &instance_id,
        activity_log_enabled,
        &owners.activity,
    );
    let (_app_outbox_path, app_outbox) = prepare_app_outbox(&config_dir, &instance_id)?;
    let ui_automation_state = crate::testability::ui_automation::UiAutomationState::new(
        ui_automation_enabled,
        config_dir.clone(),
    );

    // Generate web access token — separate from master token for limited blast radius
    let web_access_token = Arc::new(WebAccessToken::new(uuid::Uuid::new_v4().to_string()));

    println!("[master-token] {}", master_token.value());
    println!("[web-token] {}", web_access_token.value());
    println!("[app-outbox] {}", app_outbox.path());
    log::info!("[master-token] Generated (see stdout)");
    log::info!("[web-token] Generated (see stdout)");
    log::info!("[app-outbox] {} (see stdout)", app_outbox.path());

    // Write web token to a file so external tools can read it
    if let Some(token_path) = config::config_dir().map(|d| d.join("web-token.txt")) {
        let _ = std::fs::write(&token_path, web_access_token.value());
    }

    // Persist master token and app outbox path so the CLI can use them
    if let Some(dir) = config::config_dir() {
        let _ = std::fs::write(dir.join("master-token.txt"), master_token.value());
        let _ = std::fs::write(dir.join("app-outbox-path.txt"), app_outbox.path());
    }

    // Issue #231: write daemon.pid so CLI verbs can detect a dead daemon.
    config::daemon_pid::write_pid_file();

    Ok(BootPreamble {
        master_token,
        app_outbox,
        ui_automation_state,
        web_access_token,
    })
}

struct RuntimeState {
    settings: SettingsState,
    coordinator_clocks: crate::config::coordinator_clocks::CoordinatorClocksState,
    coordinator_clocks_for_exit: crate::config::coordinator_clocks::CoordinatorClocksState,
    resource_monitor_state: Arc<resource_monitor::ResourceMonitorState>,
    screenshot_capture_state: screenshot::ScreenshotCaptureState,
    screenshot_hotkey_state: screenshot::ScreenshotHotkeyState,
    settings_for_web: SettingsState,
    detached_sessions: DetachedSessionsState,
    voice_tracking: VoiceTrackingState,
    spec_board_state: SpecBoardState,
    loop_scheduler: Arc<loops::scheduler::LoopScheduler>,
    loop_scheduler_for_setup: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    non_stop_state_for_setup: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ipc_observer: Arc<crate::loops::ipc_observer::IpcObserver>,
    ipc_observer_for_setup: Arc<crate::loops::ipc_observer::IpcObserver>,
    ipc_observer_for_handler: Arc<crate::loops::ipc_observer::IpcObserver>,
    config_seed_lock: ConfigSeedLockState,
    update_check_state: UpdateCheckState,
    update_check_state_for_setup: UpdateCheckState,
    agent_update_gate: Arc<agent_update::AgentUpdateGate>,
    agent_update_gate_for_setup: Arc<agent_update::AgentUpdateGate>,
    agent_install_cache: Arc<crate::agent_version::AgentInstallCache>,
    shutdown_for_setup: ShutdownSignal,
    shutdown_for_exit: ShutdownSignal,
    selection_coordinator_for_setup: crate::session::selection::SelectionCoordinator,
    selection_coordinator_for_exit: crate::session::selection::SelectionCoordinator,
    tg_mgr_for_exit: TelegramBridgeState,
    resource_monitor_for_setup: Arc<resource_monitor::ResourceMonitorState>,
    resource_monitor_for_exit: Arc<resource_monitor::ResourceMonitorState>,
    ui_automation_state_for_setup: crate::testability::ui_automation::UiAutomationState,
    ui_automation_state_for_exit: crate::testability::ui_automation::UiAutomationState,
    message_store_state: crate::api::message_store::MessageStoreState,
    pty_target_gate_state: crate::api::message_store::PtyInputTargetGateState,
}

fn prepare_runtime_state(
    owners: &crate::AppOwners,
    response_close: &crate::pty::manager::ResponseCloseState,
    shutdown_signal: &ShutdownSignal,
    selection_coordinator: &crate::session::selection::SelectionCoordinator,
    tg_mgr: &TelegramBridgeState,
    ui_automation_state: &crate::testability::ui_automation::UiAutomationState,
) -> RuntimeState {
    let loaded_settings = config::settings::load_settings();
    response_close.apply_settings(
        loaded_settings.response_close_enabled,
        loaded_settings.response_close_idle_seconds,
        std::time::Instant::now(),
    );
    // (#621) Snapshot registered project paths for the startup orphan-clock prune
    // below (taken before the value is moved into the RwLock).
    let startup_project_paths = loaded_settings.project_paths.clone();
    let settings: SettingsState = crate::config::settings::new_settings_state(loaded_settings);
    // #552 persisted coordinator badge clock + auto-closed marker store (loaded
    // once at startup; flushed by the auto-close tick and on app exit).
    let coordinator_clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(crate::config::coordinator_clocks::load()));
    // (#621) Conservative backstop: drop clock keys for workgroups confirmed gone
    // on disk (historical orphans + CLI-removed wgs). Keep-on-any-doubt.
    crate::config::coordinator_clocks::prune_orphaned_workgroups_and_persist(
        &coordinator_clocks,
        &startup_project_paths,
    );
    let coordinator_clocks_for_exit = Arc::clone(&coordinator_clocks);
    let resource_monitor_state = Arc::new(
        resource_monitor::ResourceMonitorState::with_spawn_registry(owners.spawn.clone()),
    );
    // #714 screenshot capture lifecycle + global-hotkey registration state.
    let screenshot_capture_state: screenshot::ScreenshotCaptureState = Arc::new(
        tokio::sync::Mutex::new(screenshot::ScreenshotCaptureLifecycle::Idle),
    );
    let screenshot_hotkey_state: screenshot::ScreenshotHotkeyState = Arc::new(
        std::sync::Mutex::new(screenshot::ScreenshotHotkeyRuntime::default()),
    );
    let settings_for_web = Arc::clone(&settings);
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        commands::spec_board::SpecBoardManager::new(),
    ));
    let loop_scheduler = Arc::new(loops::scheduler::LoopScheduler::new());
    let loop_scheduler_for_setup = Arc::clone(&loop_scheduler);

    // (#777) Non-stop watchdog: timing + actuation state. Managed for the
    // `non_stop_report` command; the background loop is started in setup.
    let non_stop_state = crate::loops::non_stop_watchdog::NonStopWatchdogState::new();
    let non_stop_state_for_setup = non_stop_state.clone();

    // (#1652) IPC observer: the process-side view of the frontend -> backend
    // invoke stream. Managed for `ipc_blackbox_report`, stamped by the
    // `invoke_handler` wrapper below, and watched by the loop started in setup.
    let ipc_observer = crate::loops::ipc_observer::IpcObserver::new();
    let ipc_observer_for_setup = std::sync::Arc::clone(&ipc_observer);
    let ipc_observer_for_handler = std::sync::Arc::clone(&ipc_observer);

    // Config-seed critical-section lock. Serializes `perform_config_seed` for a
    // replica so concurrent same-replica spawns cannot clobber each other's
    // in-flight seed scratch (see `ConfigSeedLockState`).
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));

    // Issue #609 - cached "npm update available" result, set ONCE by the
    // detached startup check below and read by `get_update_status`.
    let update_check_state: UpdateCheckState = Arc::new(std::sync::OnceLock::new());
    let update_check_state_for_setup = Arc::clone(&update_check_state);

    // Issue #1327 - startup coding-agent update flow: blocks every session open
    // (via `create_session_inner_impl`) until the per-command update run
    // finishes or times out. Managed before the restore task is submitted.
    let agent_update_gate: Arc<agent_update::AgentUpdateGate> =
        Arc::new(agent_update::AgentUpdateGate::new());
    let agent_update_gate_for_setup = Arc::clone(&agent_update_gate);
    // #1551 - process-lifetime install-state cache for the coding-agent version
    // probes (Settings overview, the pass's pre-update probes and the post-pass
    // re-probes all schedule through it).
    let agent_install_cache: Arc<crate::agent_version::AgentInstallCache> =
        Arc::new(crate::agent_version::AgentInstallCache::new());

    let shutdown_for_setup = shutdown_signal.clone();
    let shutdown_for_exit = shutdown_signal.clone();
    let selection_coordinator_for_setup = selection_coordinator.clone();
    let selection_coordinator_for_exit = selection_coordinator.clone();
    let tg_mgr_for_exit = tg_mgr.clone();
    let resource_monitor_for_setup = Arc::clone(&resource_monitor_state);
    let resource_monitor_for_exit = Arc::clone(&resource_monitor_state);
    let ui_automation_state_for_setup = ui_automation_state.clone();
    let ui_automation_state_for_exit = ui_automation_state.clone();

    // One recovered operation store is shared by the filesystem poller and
    // both API start paths. A failure disables only privileged PTY input.
    let message_store_state = crate::api::message_store::MessageStoreState::initialize();
    if let Ok(store) = &message_store_state.store {
        store.bind_response_close(response_close.clone());
    } else {
        response_close.fail_closed();
    }
    let pty_target_gate_state = message_store_state.target_gate_state();

    RuntimeState {
        settings,
        coordinator_clocks,
        coordinator_clocks_for_exit,
        resource_monitor_state,
        screenshot_capture_state,
        screenshot_hotkey_state,
        settings_for_web,
        detached_sessions,
        voice_tracking,
        spec_board_state,
        loop_scheduler,
        loop_scheduler_for_setup,
        non_stop_state,
        non_stop_state_for_setup,
        ipc_observer,
        ipc_observer_for_setup,
        ipc_observer_for_handler,
        config_seed_lock,
        update_check_state,
        update_check_state_for_setup,
        agent_update_gate,
        agent_update_gate_for_setup,
        agent_install_cache,
        shutdown_for_setup,
        shutdown_for_exit,
        selection_coordinator_for_setup,
        selection_coordinator_for_exit,
        tg_mgr_for_exit,
        resource_monitor_for_setup,
        resource_monitor_for_exit,
        ui_automation_state_for_setup,
        ui_automation_state_for_exit,
        message_store_state,
        pty_target_gate_state,
    }
}
