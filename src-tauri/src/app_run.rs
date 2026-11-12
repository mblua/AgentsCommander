//! The desktop app entry: boot preamble, managed state and the Tauri builder
//! chain. The `.setup` hook lives in `setup.rs` and the run-event handler in
//! `exit.rs`.

pub(crate) mod exit;
pub(crate) mod history_environment;
mod setup;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use crate::{
    agent_update, capture, commands, config, log_naming_migration_summary, loops, network,
    preflight_config_startup, prepare_app_outbox, resource_monitor, screenshot, ApiServerHandle,
    CoManagedSupervisorHandle, ConfigSeedLockState, DetachedSessionsState, IdleDetector,
    MasterToken, OutputSenderMap, PendingSelfClear, QuitGate, RestoreInProgress, SessionManager,
    SettingsState, ShutdownSignal, SpecBoardState, StartupError, TelegramBridgeManager,
    TelegramBridgeState, UpdateCheckState, VoiceTracker, VoiceTrackingState, WebAccessToken,
    WebServerHandle, WsBroadcaster,
};

#[expect(clippy::exit, reason = "Tauri generated thread failure exits")]
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

    let l = prepare_launch(&owners, test_window_placement, ui_automation_enabled)?;
    let exit_ctx = exit::ExitCtx {
        detached_set: l.rs.detached_sessions.clone(),
        spec_board_state: l.rs.spec_board_state.clone(),
        activity_for_exit: Arc::clone(&owners.activity),
        session_mgr_for_exit: Arc::clone(&l.session_mgr),
        coordinator_clocks_for_exit: l.for_exit.coordinator_clocks_for_exit,
        shutdown_for_exit: l.for_exit.shutdown_for_exit,
        selection_coordinator_for_exit: l.for_exit.selection_coordinator_for_exit,
        tg_mgr_for_exit: l.for_exit.tg_mgr_for_exit,
        resource_monitor_for_exit: l.for_exit.resource_monitor_for_exit,
        ui_automation_state_for_exit: l.for_exit.ui_automation_state_for_exit,
    };

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
        .manage(l.boot.master_token)
        .manage(l.boot.app_outbox)
        .manage(l.session_mgr)
        .manage(l.selection_coordinator)
        .manage(l.tg_mgr)
        .manage(l.capture_registry_for_state)
        .manage(network::OutboundNetwork::new().expect("failed to build shared network clients"))
        .manage(Arc::clone(&l.rs.resource_monitor_state))
        .manage(l.rs.voice_tracking)
        .manage(l.rs.settings)
        .manage(l.idle_detector) // #552 managed type: Arc<IdleDetector>
        .manage(l.rs.coordinator_clocks) // #552 managed type: CoordinatorClocksState
        .manage(Arc::new(crate::session::purge_guard::PurgeGuard::default())) // #885
        .manage(l.rs.detached_sessions.clone())
        .manage(l.rs.spec_board_state.clone())
        .manage(l.rs.loop_scheduler.clone())
        .manage(l.rs.non_stop_state)
        .manage(l.rs.ipc_observer)
        .manage(l.boot.web_access_token.clone())
        .manage(l.broadcaster.clone())
        .manage(WebServerHandle::default())
        .manage(ApiServerHandle::default())
        .manage(l.rs.message_store_state)
        .manage(l.rs.pty_target_gate_state)
        .manage(l.rs.config_seed_lock)
        .manage(Arc::new(commands::session::PreflightState::default()))
        .manage(l.rs.update_check_state)
        .manage(l.rs.agent_update_gate)
        .manage(l.rs.agent_install_cache)
        .manage(l.boot.ui_automation_state)
        .manage(l.terminal_snapshot_state)
        .manage(l.shutdown_signal)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(PendingSelfClear::default()))
        .manage(l.rs.screenshot_capture_state) // #714
        .manage(l.rs.screenshot_hotkey_state) // #714
        .manage(crate::pty::input_activity::new_state()) // #871 substantive-input tracker
        .manage(l.response_close.clone())
        .manage(crate::pty::input_activity::new_typing_hold_state()) // #2336 typing hold
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(QuitGate::new())) // #2296 managed type: Arc<QuitGate>
        .setup(move |app| setup::setup(app, l.setup_ctx))
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
        .invoke_handler(crate::invoke_handler(l.rs.ipc_observer_for_handler))
        .build(tauri::generate_context!())
        .expect("error while building application")
        .run(crate::exit_callback(
            exit_ctx,
            commands::spec_board::spec_board_close_all,
        ));
    Ok(())
}

/// Session, idle and runtime state prepared before the Tauri builder chain.
struct Launch {
    boot: BootPreamble,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    selection_coordinator: crate::session::selection::SelectionCoordinator,
    tg_mgr: TelegramBridgeState,
    capture_registry_for_state: Arc<capture::registry::CaptureRegistry>,
    idle_detector: Arc<IdleDetector>,
    broadcaster: WsBroadcaster,
    terminal_snapshot_state: Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>,
    shutdown_signal: ShutdownSignal,
    response_close: crate::pty::manager::ResponseCloseState,
    rs: ManagedRuntime,
    for_exit: RuntimeExitParts,
    setup_ctx: setup::SetupCtx,
}

fn prepare_launch(
    owners: &crate::AppOwners,
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
) -> Result<Launch, StartupError> {
    let boot = prepare_boot(owners, ui_automation_enabled)?;

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
    let app_handle_lock = Arc::new(OnceLock::new());
    let idle_detector = IdleDetector::for_app_run(
        owners,
        &response_close,
        co_managed_supervisor_for_idle,
        &app_handle_lock,
    );
    let output_senders_for_pty = output_senders.clone();

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
        managed: rs,
        for_setup,
        for_exit,
    } = prepare_runtime_state(
        owners,
        &response_close,
        &shutdown_signal,
        &selection_coordinator,
        &tg_mgr,
        &boot.ui_automation_state,
    );
    let setup_ctx = setup::SetupCtx {
        owners: owners.clone(),
        test_window_placement,
        app_handle_lock,
        co_managed_supervisor_for_setup,
        co_managed_triggers,
        response_close_for_setup,
        session_mgr_for_git: Arc::clone(&session_mgr),
        session_mgr_for_git_sweeper: Arc::clone(&session_mgr),
        session_mgr_for_discovery: Arc::clone(&session_mgr),
        session_mgr_for_web: Arc::clone(&session_mgr),
        session_mgr_for_api: Arc::clone(&session_mgr),
        session_mgr_for_scraper: Arc::clone(&session_mgr),
        output_senders_for_pty,
        idle_detector_for_pty: Arc::clone(&idle_detector),
        idle_detector_for_setup: Arc::clone(&idle_detector),
        broadcaster_for_pty: broadcaster.clone(),
        broadcaster_for_web: broadcaster.clone(),
        web_access_token: Arc::clone(&boot.web_access_token),
        web_token_for_server: Arc::clone(&boot.web_access_token),
        settings_for_web: for_setup.settings_for_web,
        loop_scheduler_for_setup: for_setup.loop_scheduler_for_setup,
        non_stop_state_for_setup: for_setup.non_stop_state_for_setup,
        ipc_observer_for_setup: for_setup.ipc_observer_for_setup,
        update_check_state_for_setup: for_setup.update_check_state_for_setup,
        agent_update_gate_for_setup: for_setup.agent_update_gate_for_setup,
        shutdown_for_setup: for_setup.shutdown_for_setup,
        selection_coordinator_for_setup: for_setup.selection_coordinator_for_setup,
        resource_monitor_for_setup: for_setup.resource_monitor_for_setup,
        ui_automation_state_for_setup: for_setup.ui_automation_state_for_setup,
    };
    Ok(Launch {
        boot,
        session_mgr,
        selection_coordinator,
        tg_mgr,
        capture_registry_for_state,
        idle_detector,
        broadcaster,
        terminal_snapshot_state,
        shutdown_signal,
        response_close,
        rs,
        for_exit,
        setup_ctx,
    })
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
    if crate::filesystem::directories::cleanup_stale_instance_directories(&instances_dir).is_ok() {
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
    if let Some(dir) = config::config_dir() {
        let _ =
            crate::filesystem::documents::publish_web_access_token(&dir, web_access_token.value());
    }

    // Persist master token and app outbox path so the CLI can use them
    if let Some(dir) = config::config_dir() {
        let _ = crate::filesystem::master_token::publish(&dir, master_token.value());
        let _ = crate::filesystem::app_outbox_pointer::publish(&dir, app_outbox.path());
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
    managed: ManagedRuntime,
    for_setup: RuntimeSetupParts,
    for_exit: RuntimeExitParts,
}

struct ManagedRuntime {
    settings: SettingsState,
    coordinator_clocks: crate::config::coordinator_clocks::CoordinatorClocksState,
    resource_monitor_state: Arc<resource_monitor::ResourceMonitorState>,
    screenshot_capture_state: screenshot::ScreenshotCaptureState,
    screenshot_hotkey_state: screenshot::ScreenshotHotkeyState,
    detached_sessions: DetachedSessionsState,
    voice_tracking: VoiceTrackingState,
    spec_board_state: SpecBoardState,
    loop_scheduler: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ipc_observer: Arc<crate::loops::ipc_observer::IpcObserver>,
    ipc_observer_for_handler: Arc<crate::loops::ipc_observer::IpcObserver>,
    config_seed_lock: ConfigSeedLockState,
    update_check_state: UpdateCheckState,
    agent_update_gate: Arc<agent_update::AgentUpdateGate>,
    agent_install_cache: Arc<crate::agent_version::AgentInstallCache>,
    message_store_state: crate::api::message_store::MessageStoreState,
    pty_target_gate_state: crate::api::message_store::PtyInputTargetGateState,
}

fn prepare_resource_monitor_state(
    owners: &crate::AppOwners,
) -> Arc<resource_monitor::ResourceMonitorState> {
    use crate::cleanup_diagnostics::store::Diagnostics;

    let diagnostics = Arc::new(match config::config_dir() {
        None => Diagnostics::disabled(),
        Some(config_dir) => match Diagnostics::new(&config_dir) {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                log::warn!("cleanup_diagnostics init unavailable");
                Diagnostics::disabled()
            }
        },
    });
    Arc::new(
        resource_monitor::ResourceMonitorState::with_spawn_registry_and_diagnostics(
            owners.spawn.clone(),
            diagnostics,
        ),
    )
}

struct RuntimeSetupParts {
    settings_for_web: SettingsState,
    loop_scheduler_for_setup: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state_for_setup: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ipc_observer_for_setup: Arc<crate::loops::ipc_observer::IpcObserver>,
    update_check_state_for_setup: UpdateCheckState,
    agent_update_gate_for_setup: Arc<agent_update::AgentUpdateGate>,
    shutdown_for_setup: ShutdownSignal,
    selection_coordinator_for_setup: crate::session::selection::SelectionCoordinator,
    resource_monitor_for_setup: Arc<resource_monitor::ResourceMonitorState>,
    ui_automation_state_for_setup: crate::testability::ui_automation::UiAutomationState,
}

struct RuntimeExitParts {
    coordinator_clocks_for_exit: crate::config::coordinator_clocks::CoordinatorClocksState,
    shutdown_for_exit: ShutdownSignal,
    selection_coordinator_for_exit: crate::session::selection::SelectionCoordinator,
    tg_mgr_for_exit: TelegramBridgeState,
    resource_monitor_for_exit: Arc<resource_monitor::ResourceMonitorState>,
    ui_automation_state_for_exit: crate::testability::ui_automation::UiAutomationState,
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
    let resource_monitor_state = prepare_resource_monitor_state(owners);
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
        // Handle clones first; `managed` then moves the originals.
        for_setup: RuntimeSetupParts {
            settings_for_web,
            loop_scheduler_for_setup,
            non_stop_state_for_setup,
            ipc_observer_for_setup,
            update_check_state_for_setup,
            agent_update_gate_for_setup,
            shutdown_for_setup: shutdown_signal.clone(),
            selection_coordinator_for_setup: selection_coordinator.clone(),
            resource_monitor_for_setup: Arc::clone(&resource_monitor_state),
            ui_automation_state_for_setup: ui_automation_state.clone(),
        },
        for_exit: RuntimeExitParts {
            coordinator_clocks_for_exit,
            shutdown_for_exit: shutdown_signal.clone(),
            selection_coordinator_for_exit: selection_coordinator.clone(),
            tg_mgr_for_exit: tg_mgr.clone(),
            resource_monitor_for_exit: Arc::clone(&resource_monitor_state),
            ui_automation_state_for_exit: ui_automation_state.clone(),
        },
        managed: ManagedRuntime {
            settings,
            coordinator_clocks,
            resource_monitor_state,
            screenshot_capture_state,
            screenshot_hotkey_state,
            detached_sessions,
            voice_tracking,
            spec_board_state,
            loop_scheduler,
            non_stop_state,
            ipc_observer,
            ipc_observer_for_handler,
            config_seed_lock,
            update_check_state,
            agent_update_gate,
            agent_install_cache,
            message_store_state,
            pty_target_gate_state,
        },
    }
}
