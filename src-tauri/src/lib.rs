pub mod agent_update;
pub mod agent_version;
pub mod api;
pub mod capture;
pub mod cli;
pub mod commands;
pub mod config;
pub mod errors;
pub mod logging;
pub mod loops;
pub mod network;
pub(crate) mod path_identity;
pub mod path_utils;
pub mod phone;
pub mod pty;
pub mod resource_monitor;
pub mod screenshot;
pub mod session;
pub mod shutdown;
pub mod telegram;
pub mod test_support;
pub mod testability;
pub mod update_check;
pub mod voice;
pub mod web;

mod co_managed;
mod pty_wiring;
mod quit_gate;
mod reader_demand;
mod restore;
mod startup;
mod web_server;
mod window;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, OnceLock};

use commands::ac_discovery::DiscoveryBranchWatcher;
use config::sessions_persistence;
use config::settings::SettingsState;
use pty::agent_quota::AgentQuotaEngine;
use pty::context_scrape::ContextScraper;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::watchers::WatcherEngine;
use session::manager::SessionManager;
use shutdown::ShutdownSignal;
use tauri::{Emitter, Manager};
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use voice::tracker::{VoiceTracker, VoiceTrackingState};
use web::auth::WebAccessToken;
use web::broadcast::WsBroadcaster;

pub(crate) use co_managed::route::spawn_co_managed_supervisor;
#[cfg(test)]
pub(crate) use co_managed::CoManagedTestHooks;
pub(crate) use co_managed::{
    abstain_reason_label, co_managed_catalog, co_managed_jev_settings, co_managed_state_reason,
    co_managed_user_message, emit_co_managed_state, emit_session_idle_edge, route_co_managed_wake,
    surface_co_managed_user_message, write_co_managed_message_file, CoManagedRoute,
    CoManagedSupervisorHandle, CoManagedTrigger, CO_MANAGED_MAX_ATTEMPTS,
    CO_MANAGED_RETRY_BACKOFF_MS,
};
pub(crate) use pty_wiring::{
    codex_activity_factory, QuotaRows, QuotaSink, QuotaSources, ScraperPatterns, ScraperPersist,
    ScraperRows, ScraperSamples, ScraperSink, WatcherBackends, WatcherDelivery, WatcherPatterns,
    WatcherSink,
};
pub use quit_gate::{
    apply_quit_gate_effects, quit_gate_run, quit_gate_window_gone, QuitAbortReason, QuitGate,
    QuitGateClock, QuitGateEffects, QuitGateHost, QuitGateRegistration, QuitOutcome,
    QuitOutcomeKind, TauriQuitHost, TokioQuitClock, QUIT_GATE_MAIN_LABEL, QUIT_GATE_TIMEOUT,
};
pub(crate) use quit_gate::{
    quit_application, quit_gate_progress, quit_gate_register, quit_gate_resolve,
    quit_gate_unregister,
};
pub(crate) use reader_demand::{
    reader_demand_pass, recovery_pass, watch_capture_slots, READER_DEMAND_TICK_SECS,
};
#[cfg(test)]
pub(crate) use reader_demand::{tick_raise_calls, tick_recheck_calls};
pub(crate) use restore::native::start_native_restore_startup;
pub(crate) use restore::{
    load_startup_sessions, normalize_persisted_active_flags, resolve_is_coord_for_restore,
    restore_session_should_become_active, restore_session_should_wake,
    should_auto_create_root_agent_on_first_restore, should_wake_root_agent_on_restore,
    skip_auto_resume_for_restore, spawn_restore_startup, PersistedActiveFlagNormalization,
    RestoreObserverStartBarrier,
};
pub(crate) use startup::prepare_app_outbox;
pub use startup::{log_naming_migration_summary, preflight_config_startup, StartupError};
pub(crate) use web_server::WEB_SERVER_START_CANCELLED;
pub use web_server::{
    ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle,
    WebServerLifecycleSnapshot, WebServerStartWaiter, WebServerStopWaiter,
};
pub(crate) use window::{
    apply_main_display_state, centered_default_main_geometry, effective_main_display_state,
    is_visible_on_monitors, physical_to_logical, response_close_wiring_matches,
};

// Test-only names the `tests` tree reaches through `super::*`.
#[cfg(test)]
pub(crate) use co_managed::route::{
    co_managed_cycle, commit_co_managed_effect, handle_co_managed_trigger,
    resolve_co_managed_route, CoManagedOutcome,
};
#[cfg(test)]
pub(crate) use co_managed::{
    co_managed_candidate_label, utf8_safe_excerpt, CO_MANAGED_EXCERPT_BYTES,
};
#[cfg(test)]
use pty::agent_quota::source::SourceSpec;
#[cfg(test)]
use pty::agent_quota::{AgentQuotaSource, QuotaSourceProvider};
#[cfg(test)]
use pty::context_scrape::{ContextPatternSource, ContextSample, ContextSampleSink};
#[cfg(test)]
use pty::watchers::{WatcherEventSink, WatcherPatternSource};
#[cfg(test)]
pub(crate) use reader_demand::{
    release_room_reader_demands_when_globally_off, reraise_room_reader_demands,
};
#[cfg(test)]
pub(crate) use restore::{should_wake_on_restore, should_wake_working_agent_on_restore};
#[cfg(test)]
pub(crate) use startup::{
    preflight_config_startup_for, preflight_config_startup_in, take_naming_migration_summary,
    StartupErrorKind, INSTANCE_FAMILIES_RENAMES,
};

/// Snapshot scanner terminality is deliberately not an input here.
///
/// A retained scanner task publishes a response file; its only `SessionManager`
/// access is a read-lock clone, so it cannot leave session state inconsistent
/// and cannot make a session snapshot wrong. Gating persistence on it was not a
/// rare edge either: `SNAPSHOT_SERVER_TIMEOUT` is twice
/// `SHUTDOWN_CLEANUP_BUDGET_SECS`, and the drain correctly refuses to abort
/// owned or finalizer tasks, so any snapshot admitted inside the shutdown window
/// and running near its own legitimate deadline suppressed persistence and cost
/// the user their session list. Retained scanner work is still reported in the
/// shutdown diagnostics.
pub(crate) fn shutdown_persistence_allowed(
    selection_persistence_safe: bool,
    container_cleanup_terminal: bool,
) -> bool {
    selection_persistence_safe && container_cleanup_terminal
}

#[cfg(test)]
pub(crate) fn combined_shutdown_retained_diagnostics(
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    combined_shutdown_retained_diagnostics_with_scanner(
        Vec::new(),
        selection_retained,
        container_retained,
    )
}

pub(crate) fn combined_shutdown_retained_diagnostics_with_scanner(
    scanner_retained: Vec<String>,
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    let scanner = scanner_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "terminalSnapshotScanner",
            context,
        )
    });
    let selection = selection_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
    });
    let container = container_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "containerShutdown",
            context,
        )
    });
    crate::pty::container_runtime::cap_retained_owner_diagnostics(
        scanner.chain(selection).chain(container),
    )
}

fn remove_container_route_until(
    weak_pty_mgr: &std::sync::Weak<Mutex<PtyManager>>,
    session_id: uuid::Uuid,
    deadline: std::time::Instant,
) -> Result<(), crate::pty::container_backend::RouteRemovalError> {
    let Some(pty_mgr) = weak_pty_mgr.upgrade() else {
        return Ok(());
    };
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        match pty_mgr.try_lock() {
            Ok(pty_manager) => match pty_manager.try_remove_route_if_kind(
                session_id,
                crate::pty::backend::SessionBackendKind::ContainerTransport,
            ) {
                Ok(()) => return Ok(()),
                Err(crate::pty::manager::PtyRouteRemovalError::LockPoisoned) => {
                    return Err(
                        crate::pty::container_backend::RouteRemovalError::LockPoisoned(
                            "ptyRouteRegistry",
                        ),
                    );
                }
                Err(crate::pty::manager::PtyRouteRemovalError::Busy) => {}
            },
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(
                    crate::pty::container_backend::RouteRemovalError::LockPoisoned("ptyManager"),
                );
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2).min(remaining));
    }
}

pub(crate) fn install_container_route_remover(pty_mgr: &Arc<Mutex<PtyManager>>) {
    let weak_pty_mgr = Arc::downgrade(pty_mgr);
    let container_backend = pty_mgr.lock().unwrap().container_backend();
    container_backend.set_route_remover(Arc::new(move |session_id, deadline| {
        remove_container_route_until(&weak_pty_mgr, session_id, deadline)
    }));
}

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

/// Serializes the config-seed critical section (`perform_config_seed`) for a
/// replica, so two concurrent same-replica spawns cannot clobber each other's
/// in-flight `<dest>.acseed-*` scratch during `clear_stale_seed_scratch`
/// (grinch HIGH-1; see `config/config_seed.rs` CONCURRENCY CONTRACT). Acquired
/// in `commands::session.rs` around the seed swap.
pub type ConfigSeedLockState = Arc<tokio::sync::Mutex<()>>;

// Issue #609 - cached "npm update available" result. Set ONCE by the startup
// check task; read by `get_update_status` so a late-mounting sidebar still
// sees a pending update.
pub type UpdateCheckState = Arc<std::sync::OnceLock<update_check::UpdateInfo>>;

/// Floating spec/Mermaid board document state.
pub type SpecBoardState = Arc<tokio::sync::RwLock<commands::spec_board::SpecBoardManager>>;

/// #632 - hard ceiling on the shutdown reaper cleanup. For jobbed sessions the Job
/// Object kill already prevented orphans, so exceeding this just stops the
/// best-effort accounting reaper. For a job-less session (assign failed) this bound
/// CAN abandon a still-dying tree, so the Exit handler warns when that is possible
/// (MED-2).
const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;

/// Master token generated at app startup. Allows bypassing team validation (can_reach).
/// Persisted to `master-token.txt` in config_dir for CLI use. Regenerated on each app startup. See
/// #34.
/// Field is private — use `matches()` for constant-time comparison.
pub struct MasterToken(String);

impl MasterToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Constant-time comparison to prevent timing oracle attacks.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
    }

    /// Display value (for printing to stdout at startup only).
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// §224 A.2.5 / G1 — set true while the post-startup session-restore loop is
/// running (`lib.rs` setup task that calls `create_session_inner` for every
/// persisted session). Read by `mailbox::handle_close_session` to decide
/// whether `session_ids.is_empty()` means "no live session for this FQN" or
/// "restore loop hasn't reached this session yet — retry briefly."
pub struct RestoreInProgress(pub AtomicBool);

/// (#617/#668) Sessions with a self context operation awaiting their sustained-idle
/// window. Insert on queue; remove when the deferred task completes, the session
/// dies, or the safety cap expires. A session_id already present means a repeat
/// self operation is a no-op ("already_queued") - requests never stack.
/// In-memory only: a daemon restart drops pending requests (accepted, best-effort).
///
/// Newtype is mandatory: `DetachedSessionsState` (lib.rs:38) is already a managed
/// bare `Arc<Mutex<HashSet<Uuid>>>`, and Tauri keys managed state by Rust type, so
/// a second bare alias would collide. Mirrors `RestoreInProgress`.
#[derive(Default)]
pub struct PendingSelfClear(pub Mutex<HashSet<uuid::Uuid>>);

/// Instance-private outbox directory. Only this app instance polls it.
/// Created at startup, path printed to stdout alongside master token.
pub struct AppOutbox(String);

impl AppOutbox {
    pub fn new(path: String) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &str {
        &self.0
    }
}

/// (#1652) Pins the type of the closure `generate_handler!` produces. The
/// macro's closure parameter is UNANNOTATED and rustc type-checks the body at
/// the `let` that binds it. A bare binding is therefore
/// `error[E0282]: type annotations needed`, and annotating the OUTER closure's
/// parameter does not reach it. This identity function supplies the expected
/// type on the binding itself. Preferred over
/// `let generated: Box<dyn Fn(..) -> bool + Send + Sync> = ...` because
/// `pty_write` is the highest-frequency command and boxing adds an indirection
/// to every invoke.
fn pin_handler<F: Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static>(f: F) -> F {
    f
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
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

    preflight_config_startup()?;

    // Same backend the CLI path now installs in `main.rs` — see `logging.rs`
    // for the rationale. Idempotent, so a hypothetical second call (or the
    // CLI path having already run in this process) is a no-op.
    crate::logging::init_logger();
    log_naming_migration_summary();

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
    crate::config::activity_log::init_run(&config_dir, &instance_id, activity_log_enabled);
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

    // Create WS broadcaster (shared between Tauri commands and web server)
    let broadcaster = WsBroadcaster::new();

    let response_close = crate::pty::manager::ResponseCloseState::default();
    let response_close_for_setup = response_close.clone();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        SessionManager::with_response_close(response_close.clone()),
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
    let (co_managed_supervisor, co_managed_triggers) = CoManagedSupervisorHandle::new();
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
    let idle_detector = IdleDetector::new(
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
    let resource_monitor_state = Arc::new(resource_monitor::ResourceMonitorState::new());
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

    builder
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
        .manage(std::sync::Arc::new(crate::session::purge_guard::PurgeGuard::default())) // #885
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
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

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
                    match crate::screenshot::register_configured_hotkey(&app_for_hotkey, &configured)
                    {
                        Ok(()) => log::info!(
                            "[screenshot] global hotkey registered '{}'",
                            configured
                        ),
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
            crate::logging::spawn_error_emit_task(app.handle().clone());

            // #271 — seed `<config_dir>/agent-templates/` + README on startup.
            crate::commands::role_templates::ensure_default_templates_dir_at_config();

            // (#621) GC the context-cache: unlink generated *-context-*.md files
            // older than the retention window. Cleans orphans from removed
            // workgroups AND caps the unbounded-growth secondary finding. Robust +
            // self-healing: live agents re-write their cache every launch.
            crate::config::session_context::sweep_context_cache_at_startup();

            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            app.manage(Arc::clone(&discovery_branch_watcher));

            // #1298 - the single global POLLING producer of per-repo git state. Both
            // watchers read its published snapshot instead of spawning `git` themselves.
            //
            // Started HERE and not inside `restore_observer_barrier` for exactly one
            // reason: that barrier gates observers which mutate session metadata or
            // persistence, and this one mutates neither, it only publishes into
            // process-local maps. The construction point buys no head start by itself,
            // because at this point there is nothing to sweep (sessions are restored
            // below, discovery is frontend-driven). Cold start is governed by the
            // empty-round floor instead (plan D3). Do not move this under the barrier, and
            // do not "restore" a head-start rationale that was never true.
            let git_sweeper = crate::pty::git_watcher::GitSweeper::new(
                session_mgr_for_git_sweeper,
                app.state::<SettingsState>().inner().clone(),
            );
            git_sweeper.start(shutdown_for_setup.clone());

            // #2064 - the remote activity producer: asks GitHub, through `gh`,
            // whether CI runs on each room repo's exact HEAD and whether the
            // default branch is ahead. Beside `GitSweeper` for the same reason it
            // sits there: it mutates no session metadata, it only publishes into
            // process-local maps. Phase B consumes the transition stream: the
            // notifier started below is its only consumer.
            let (remote_sweeper, remote_transitions) =
                crate::pty::remote_watcher::RemoteSweeper::new(
                    app.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                        .inner()
                        .clone(),
                    app.state::<SettingsState>().inner().clone(),
                    Box::new({
                        let app_for_remote_activity = app.handle().clone();
                        move |payload: &crate::pty::remote_watcher::RemoteActivityPayload| {
                            let _ = app_for_remote_activity
                                .emit("ac_remote_activity_updated", payload);
                        }
                    }),
                    // #2374 - the neutral snapshot destination: the instance
                    // config directory, resolved here and never inferred from
                    // settings or another artifact.
                    crate::pty::remote_watcher::RemoteSweeper::production_seams(
                        crate::config::config_dir(),
                    ),
                );
            let _ = remote_sweeper.start(shutdown_for_setup.clone());
            // #2064 Phase B - the transition stream's consumer. Started beside the
            // sweeper that produces it, in this same setup closure, and handed a
            // child of the same shutdown token, so the notifier stops when the app
            // does. It injects only into a room orchestrator, and only when both
            // its feature dial and its notify dial are on.
            // The JoinHandle is dropped on purpose: the monitor's lifecycle is its
            // shutdown token and the sweeper's channel, not a join at exit.
            drop(crate::session::remote_alerts::start(
                app.handle().clone(),
                remote_transitions,
                shutdown_for_setup.token().child_token(),
            ));

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                Arc::clone(&git_watcher),
                Some(broadcaster_for_pty),
                Some(selection_coordinator_for_setup.container_lifecycle_sender()),
            )));
            pty_mgr.lock().unwrap().bind_response_close(response_close_for_setup.clone());
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
                    crate::agent_update::run_startup_updates(app_for_agent_updates, gate_for_setup)
                        .await;
                });
            }

            selection_coordinator_for_setup
                .start(app.handle().clone())
                .map_err(|error| error.to_string())?;
            let restore_observer_barrier = RestoreObserverStartBarrier::default();
            let restore_barrier = tauri::async_runtime::block_on(
                selection_coordinator_for_setup.submit_restore_first(),
            )?;
            restore_observer_barrier.mark_restore_admitted()?;
            let restore_transaction = restore_barrier.transaction(app.handle().clone());
            app.state::<Arc<RestoreInProgress>>()
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);

            // #1056 context-alert actor. Start it before the scraper so the bounded sender
            // exists before the first sample; manage it for final joined shutdown.
            let context_alert_monitor =
                crate::session::context_alerts::ContextAlertMonitor::start(
                    app.handle().clone(),
                    shutdown_for_setup.token().child_token(),
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
            if app.try_state::<crate::pty::input_activity::TypingHoldState>().is_some()
                && app.try_state::<crate::pty::manager::ResponseCloseState>().is_some_and(|bound| bound.same_state(&response_close_for_setup)) {
                response_close_for_setup.bind_publisher(crate::pty::manager::ResponseClosePublisher::Hold);
            }
            let manager = tauri::async_runtime::block_on(async {
                let managed = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let guard = managed.read().await;
                guard.clone()
            });
            let response_close_wired = {
                let pty = pty_mgr.lock().unwrap();
                response_close_wiring_matches(&response_close_for_setup, &manager, &pty, &idle_detector_for_setup, &menu_guard)
            };
            if !response_close_wired || response_close_for_setup.try_enable_readiness().is_err() {
                response_close_for_setup.fail_closed();
                log::warn!("[response-close] safety publisher wiring unavailable; readiness remains disabled");
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
                    crate::update_check::run_remote_blocking_menus_startup(
                        app_handle_for_remote_menus,
                    )
                    .await;
                });
            }
            // #2133 - detached remote per-agent help download. Fail-silent; applies at the next
            // start.
            {
                let app_handle_for_remote_agent_help = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::update_check::run_remote_agent_help_startup(
                        app_handle_for_remote_agent_help,
                    )
                    .await;
                });
            }

            if let Err(e) = crate::config::root_agent::ensure_root_agent_dir() {
                log::error!("[root-agent] Failed to provision root agent directory: {}", e);
            }

            // #1157 - seed and reconcile the operator-editable injected-message
            // templates. Best effort, never fatal, matching the block above.
            //
            // The ordering is NOT load-bearing: `render` resolves from the
            // registry or the embedded default and never fails, so an alert
            // firing before this point still renders correctly. The cost is
            // bounded and small (one read plus at most three small atomic
            // writes on the setup thread), not "non-blocking"; moving it to
            // tokio::spawn would buy nothing and would race the registry load.
            if let Some(dir) = crate::config::config_dir() {
                if let Err(e) = crate::config::injected_messages::ensure_injected_messages(&dir) {
                    log::warn!("[injected-messages] provisioning failed: {}", e);
                }
            }
            // Force the registry once, so the first alert - whose line() runs
            // on a tokio worker - never performs a blocking read there.
            let _ = crate::config::injected_messages::registry();

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
                    sessions_persistence::load_sessions_purging_outside_project_paths(paths, &restore_settings_snapshot),
                )
            });
            if let Err(error) = &persisted {
                log::error!("[restore] Project state unavailable; session load/purge skipped: {}", error);
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
            let restore_flag = app
                .state::<Arc<RestoreInProgress>>()
                .inner()
                .clone();
            restore_flag
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
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
                log::info!("[window-setup]   monitor {}: ({}, {}) -> ({}, {}) scale={}", i, mx, my, mx2, my2, scale);
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
                    (s.width as f64 / primary_scale, s.height as f64 / primary_scale)
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
            let default_main =
                centered_default_main_geometry(primary_x, primary_y, screen_w, screen_h);

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
                    "{{\"event\":\"testWindowInfo\",\"pid\":{},\"position\":{},\"size\":{},\"maximized\":{}}}",
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
                    use windows_sys::Win32::Graphics::Gdi::{
                        GetMonitorInfoW, MonitorFromRect, MONITORINFO,
                        MONITOR_DEFAULTTONEAREST,
                    };
                    use windows_sys::Win32::Foundation::{POINT, RECT};
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetWindowPlacement, IsZoomed, SetWindowPlacement, SetWindowPos,
                        ShowWindow, WINDOWPLACEMENT, SWP_NOACTIVATE, SWP_NOZORDER,
                        SWP_SHOWWINDOW, SW_RESTORE, SW_SHOWMAXIMIZED,
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
                                    log::warn!(
                                        "[test-window] GetWindowPlacement failed before maximize"
                                    );
                                }
                                placement.length =
                                    std::mem::size_of::<WINDOWPLACEMENT>() as u32;
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

                if let Err(e) = win.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                    width,
                    height,
                })) {
                    log::warn!("[test-window] failed to set physical size: {}", e);
                }
                if let Err(e) = win.set_position(tauri::Position::Physical(
                    tauri::PhysicalPosition { x, y },
                )) {
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
                            geo.x, geo.y, geo.width, geo.height,
                            logical.x, logical.y, logical.width, logical.height
                        );
                        logical
                    }
                    Some(geo) => {
                        log::warn!(
                            "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, \
                                falling back to centered default",
                            geo.x, geo.y, geo.width, geo.height
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
                let session_mgr_clone = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>().inner().clone();
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
                let restore_flag_for_task = app
                    .state::<Arc<RestoreInProgress>>()
                    .inner()
                    .clone();
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
                );
            }

            Ok(())
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
        .invoke_handler({
            let observer = ipc_observer_for_handler;
            let generated = pin_handler(tauri::generate_handler![
                commands::session::create_session,
                commands::session::destroy_session,
                commands::session::close_coordinator,
                commands::session::restart_session,
                commands::session::resolve_blocking_menu,
                commands::session::switch_session,
                commands::session::rename_session,
                commands::session::set_last_prompt,
                commands::session::list_sessions,
                commands::session::get_active_session,
                commands::session::get_last_agent_message,
                session::warnings::drain_session_warnings,
                quit_gate_register,
                quit_gate_unregister,
                quit_gate_resolve,
                quit_gate_progress,
                quit_application,
                commands::session::create_root_agent_session,
                commands::task::task_get_title,
                commands::task::task_set_title,
                  commands::task::task_clean,
                commands::task::task_clean_at,
                commands::task::task_set_title_at,
                commands::pty::pty_write,
                commands::pty::get_typing_hold,
                commands::pty::toggle_typing_hold,
                commands::pty::pty_resize,
                commands::pty::get_screen_snapshot,
                commands::pty::activate_terminal_output,
                commands::pty::detach_terminal_output,
                commands::pty::get_session_context,
                commands::pty::get_agent_quota_readings,
                commands::pty::get_watcher_activity,
                commands::pty::preview_watcher_pattern,
                commands::pty::preview_watcher_reach,
                commands::config::get_settings,
                commands::config::list_agent_file_sources,
                commands::config::read_agent_file_source,
                commands::config::save_agent_file_source,
                commands::config::remember_agent_file_source,
                commands::config::get_agent_help,
                commands::config::get_coding_agent_catalog,
                commands::config::get_coding_agent_welcome_status,
                commands::config::install_coding_agent,
                commands::config::get_coding_agent_catalog_report,
                commands::config::list_reseedable_agent_commands,
                commands::config::reseed_coding_agent_default,
                commands::config::update_settings,
                commands::resource_monitor::get_resource_snapshot,
                commands::resource_monitor::kill_resource_group,
                commands::config::save_settings_draft,
                commands::config::set_terminal_snapshots_enabled,
                commands::config::update_coding_agent_profiles,
                commands::config::update_coding_agent_env_settings,
                commands::config::set_agent_default_profile,
                commands::config::set_instance_profile_override,
                commands::config::resolve_coding_agent_profile,
                commands::config::preview_coding_agent_profile_selection,
                commands::config::apply_coding_agent_profile_selection,
                commands::config::preview_selection_lock_removal,
                commands::config::apply_selection_lock_removal,
                commands::config::get_replica_selection_default,
                commands::config::set_replica_selection_default,
                commands::config::set_sounds_enabled,
                commands::config::set_theme_light,
                commands::config::set_main_resource_monitor_attached,
                commands::config::set_rail_collapse,
                commands::config::move_coding_agent,
                commands::config::reorder_coding_agent,
                commands::config::set_log_level,
                commands::config::get_update_status,
                commands::config::get_agent_update_status,
                commands::config::agent_update_answer,
                commands::config::agent_update_cancel,
                commands::config::agent_updates_cancel_all,
                commands::config::get_agent_update_overview,
                commands::co_managed::co_managed_get,
                commands::co_managed::co_managed_set_enabled,
                commands::co_managed::co_managed_effective_state,
                commands::repos::search_repos,
                commands::repos::git_remote_url,
                commands::telegram::telegram_attach,
                commands::telegram::telegram_detach,
                commands::telegram::telegram_list_bridges,
                commands::telegram::telegram_get_bridge,
                commands::telegram::telegram_send_test,
                commands::telegram::telegram_send_image,
                commands::testability::ui_automation_enabled,
                commands::testability::ui_automation_frontend_ready,
                commands::testability::ui_automation_complete,
                commands::window::detach_terminal,
                commands::window::attach_terminal,
                commands::window::list_detached_sessions,
                commands::window::set_detached_geometry,
                commands::window::set_watchers_geometry,
                commands::window::set_main_window_placement,
                commands::window::open_in_explorer,
                commands::window::open_spec_board_window,
                commands::window::open_resource_monitor_window,
                commands::window::dock_resource_monitor_window,
                commands::window::open_watchers_window,
                commands::window::get_watchers_scope,
                commands::window::open_external_url,
                commands::window::focus_main_window,
                commands::spec_board::spec_board_new,
                commands::spec_board::spec_board_pick_open,
                commands::spec_board::spec_board_open,
                commands::spec_board::spec_board_save,
                commands::spec_board::spec_board_pick_save,
                commands::spec_board::spec_board_update_content,
                commands::spec_board::spec_board_list_snapshots,
                commands::spec_board::spec_board_checkout_snapshot,
                commands::spec_board::spec_board_apply_external,
                commands::spec_board::spec_board_keep_mine,
                commands::spec_board::spec_board_close,
                commands::voice::voice_transcribe,
                commands::voice::voice_mark_recording,
                commands::voice::voice_had_typing,
                commands::config::save_debug_logs,
                commands::config::append_rail_diagnostic,
                commands::config::drain_error_logs,
                commands::config::get_error_popup_mutes,
                commands::config::set_error_popup_muted,
                commands::config::open_web_remote,
                commands::config::start_api_server,
                commands::config::stop_api_server,
                commands::config::api_server_status,
                commands::config::mint_api_client,
                commands::config::start_web_server,
                commands::config::stop_web_server,
                commands::config::get_web_server_status,
                commands::config::get_web_server_owned_status,
                commands::config::list_web_server_interfaces,
                commands::config::get_instance_label,
                commands::config::fetch_home_markdown,
                commands::agent_creator::pick_folder,
                commands::agent_creator::create_agent_folder,
                commands::ac_discovery::task_get_snapshot,
                commands::ac_discovery::task_get_snapshot_at,
                commands::ac_discovery::discover_ac_agents,
                commands::ac_discovery::check_project_path,
                commands::ac_discovery::create_ac_project,
                commands::ac_discovery::open_project,
                commands::ac_discovery::new_project,
                commands::ac_discovery::remove_project,
                commands::ac_discovery::archive_project,
                commands::ac_discovery::unarchive_project,
                commands::ac_discovery::list_archived_projects,
                commands::ac_discovery::discover_project,
                commands::ac_discovery::get_project_file_difference,
                commands::project_settings::get_project_groups,
                commands::project_settings::update_project_groups,
                commands::non_stop::non_stop_report,
                commands::ipc_blackbox::ipc_blackbox_report,
                commands::ac_discovery::keep_custom_context_template,
                commands::ac_discovery::overwrite_context_template_with_default,
                commands::ac_discovery::get_replica_context_files,
                commands::ac_discovery::set_replica_context_files,
                commands::loops::create_loop,
                commands::loops::update_loop,
                commands::loops::delete_loop,
                commands::loops::toggle_loop,
                commands::loops::run_loop_now,
                commands::loops::get_loop_config,
                commands::loops::list_unresolved_loop_targets,
                commands::loops::preview_loop_cron,
                commands::entity_creation::create_agent_matrix,
                commands::entity_creation::delete_agent_matrix,
                commands::entity_creation::list_all_agents,
                commands::entity_creation::create_team,
                commands::entity_creation::delete_team,
                commands::entity_creation::update_team,
                commands::entity_creation::get_team_config,
                commands::entity_creation::create_workgroup,
                commands::entity_creation::delete_workgroup,
                commands::role_templates::list_role_templates,
                commands::role_templates::get_agency_templates_status,
                commands::role_templates::update_agency_templates,
                commands::screenshot::screenshot_get_overlay_state,
                commands::screenshot::screenshot_confirm_selection,
                commands::screenshot::screenshot_cancel_capture,
                commands::screenshot::screenshot_get_hotkey_status,
                commands::screenshot::screenshot_reload_hotkey,
            ]);
            move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
                observer.note_invoke(invoke.message.command());
                generated(invoke)
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building application")
        .run({
            let detached_set = detached_sessions.clone();
            let spec_board_state = spec_board_state.clone();
            move |app_handle, event| match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Destroyed,
                    ..
                } => {
                    // #2296 - a gate window that dies without answering must not
                    // hold a quit round open. Synchronous: it only drops the
                    // registration, clears busy and (if that gate was still
                    // unanswered) aborts the round with reason `destroyed`.
                    if let Some(gate) = app_handle.try_state::<Arc<QuitGate>>() {
                        let gate = Arc::clone(gate.inner());
                        quit_gate_window_gone(&gate, &TauriQuitHost::new(app_handle.clone()), &label);
                    }
                    // #1363 - a destroyed window's terminal-output attachments are released
                    // here, in the backend, without any frontend cooperation. It is what keeps
                    // a window that died without detaching from leaving a session emitting to
                    // a webview that is gone, and it is why the frontend close hook does not
                    // need to block the close.
                    if let Some(pty_mgr) = app_handle
                        .try_state::<Arc<Mutex<crate::pty::manager::PtyManager>>>()
                        .map(|state| Arc::clone(state.inner()))
                    {
                        let destroyed = label.clone();
                        tauri::async_runtime::spawn(async move {
                            pty_mgr
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .release_window_attachments(&destroyed);
                        });
                    }
                    if label == "spec-board" {
                        let state = spec_board_state.clone();
                        tauri::async_runtime::spawn(async move {
                            commands::spec_board::spec_board_close_all(state).await;
                        });
                    }
                    // #566 - when the main window is destroyed (any close path:
                    // X, Alt+F4, programmatic, silent- or confirm-quit), close the
                    // Resource Monitor window so it cannot orphan and keep the app
                    // alive. No-op if it was never opened or already closed.
                    if label == "main" {
                        if let Some(rm) = app_handle.get_webview_window("resource-monitor") {
                            // G4: log on failure rather than swallow. A swallowed
                            // error would hide the exact orphan bug this fixes;
                            // mirrors the FE quit path's console.warn.
                            if let Err(e) = rm.destroy() {
                                log::warn!("[shutdown] RM window destroy failed: {e}");
                            }
                        }
                        // #2563 - same orphan class for detached terminals when
                        // main is destroyed without a quit round (OS logoff,
                        // programmatic destroy). Leaves `was_detached` intact.
                        let mut destroyed = 0usize;
                        for (label, window) in app_handle.webview_windows() {
                            if !label.starts_with("terminal-") {
                                continue;
                            }
                            if let Err(e) = window.destroy() {
                                log::warn!(
                                    "[shutdown] detached window destroy failed: {label}: {e}"
                                );
                            }
                            destroyed += 1;
                        }
                        if destroyed > 0 {
                            log::info!(
                                "[shutdown] main destroyed: destroyed {} detached terminal \
                                    window(s)",
                                destroyed
                            );
                        }
                    }
                    // Detached-window destroyed (by any mechanism — X, Alt+F4, programmatic).
                    // Two jobs:
                    //   1) Clear from `DetachedSessionsState` — switch_session needs an
                    //      accurate view of which sessions have live windows.
                    //   2) Emit `terminal_attached` — frontend stores subscribed to this event
                    //      clear the id from `sessionsStore.detachedIds` (Phase 2+ only;
                    //      Phase 1 has no subscriber — the event is harmlessly dropped).
                    //
                    // DELIBERATELY ABSENT: we do NOT call `SessionManager::set_was_detached`
                    // here. That mutation is reserved for `detach_terminal_inner` (→true)
                    // and `attach_terminal` (→false) under Fix A (plan §A3.2 / NEW-3).
                    // Mirroring the clear here would reintroduce NEW-1: A3.7 quit path
                    // destroys every detached window → Destroyed fires N times → all
                    // `Session::was_detached` flipped to false → `persist_current_state`
                    // on `RunEvent::Exit` writes was_detached=false for every session →
                    // restart restores nothing detached. See plan §10 rule.
                    if let Some(id_no_dashes) = label.strip_prefix("terminal-") {
                        if id_no_dashes.len() == 32 {
                            let formatted = format!(
                                "{}-{}-{}-{}-{}",
                                &id_no_dashes[0..8],
                                &id_no_dashes[8..12],
                                &id_no_dashes[12..16],
                                &id_no_dashes[16..20],
                                &id_no_dashes[20..32],
                            );
                            if let Ok(uuid) = uuid::Uuid::parse_str(&formatted) {
                                {
                                    let mut set = detached_set.lock().unwrap();
                                    set.remove(&uuid);
                                }
                                let _ = tauri::Emitter::emit(
                                    app_handle,
                                    "terminal_attached",
                                    serde_json::json!({ "sessionId": formatted }),
                                );
                            }
                        }
                    }
                    // #714 screenshot overlay destroyed (user close, crash, or our
                    // own teardown): clear capture state and destroy sibling
                    // overlays. Idempotent — a no-op once state is already Idle.
                    if label.starts_with("screenshot-overlay-") {
                        let label = label.to_string();
                        let app = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::screenshot::handle_overlay_window_destroyed(app, label).await;
                        });
                    }
                }
                tauri::RunEvent::Exit => {
                    // Cancel all active Telegram bridges before general shutdown
                    let bridge_shutdowns = {
                        let mut tg = tauri::async_runtime::block_on(tg_mgr_for_exit.lock());
                        tg.cancel_all()
                    };
                    for shutdown in bridge_shutdowns {
                        shutdown.abort_now();
                    }

                    // #1149 - close every open activity interval here. `trigger()`
                    // below stops the IdleDetector before anything else, so this
                    // is the only position where the session map is still
                    // populated AND the detector is still alive. Without it a
                    // clean exit would drop every open interval, which is the
                    // defect this issue names first.
                    //
                    // Reaching the manager needs the outer lock, and `block_on`
                    // is forbidden on this path, so take it with `try_read`, clone
                    // the manager out (it is an `Arc` over its own state) and drop
                    // the guard before spinning.
                    let manager_for_activity = session_mgr_for_exit
                        .try_read()
                        .ok()
                        .map(|guard| guard.clone());
                    let working_snapshot = match manager_for_activity {
                        Some(manager) => {
                            // Bounded at 500 ms, and it holds no lock the writer
                            // needs while it sleeps, so it can never starve the
                            // writer it waits on. It can only fail to observe a
                            // gap, and that failure is already correct: the
                            // consumer closes every open interval at `app_stop`'s
                            // timestamp regardless of enumeration.
                            let deadline = std::time::Instant::now()
                                + std::time::Duration::from_millis(500);
                            loop {
                                if let Some(rows) = manager.try_snapshot_working_sessions() {
                                    break Some(rows);
                                }
                                if std::time::Instant::now() >= deadline {
                                    break None;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(2));
                            }
                        }
                        // The outer lock has no production writer, so unreachable
                        // in practice; falls through to the degraded path.
                        None => None,
                    };
                    let activity_batch = match &working_snapshot {
                        Some(rows) => {
                            let mut batch: Vec<_> = rows
                                .iter()
                                .map(|row| {
                                    crate::config::activity_log::build_idle_from_snapshot(
                                        row,
                                        crate::config::activity_log::IdleReason::AppStop,
                                    )
                                })
                                .collect();
                            batch.push(crate::config::activity_log::build_app_stop(
                                true,
                                rows.len(),
                            ));
                            batch
                        }
                        // The degraded path is a designed outcome, not a fallback
                        // to optimise away: the spin may legitimately exhaust
                        // under teardown load, and the enumerated records are pure
                        // precision on top of a close that happens either way.
                        None => vec![crate::config::activity_log::build_app_stop(false, 0)],
                    };
                    // One open, write and close for all N+1 lines.
                    crate::config::activity_log::append_batch(&activity_batch);

                    // #632 B1 - trigger background-task shutdown FIRST so the resource
                    // watchdog stops dispatching NEW ticks and the idle detectors stop.
                    // (An already-dispatched spawn_blocking kill_group still runs;
                    // safety there rests on B2b's bounded set + kill_group's
                    // Terminating/Terminated idempotency guard, not on trigger().)
                    let snapshot_scanner_shutdown = phone::mailbox::MailboxPoller::
                        active_terminal_snapshot_shutdown_owner();
                    if let Some(owner) = &snapshot_scanner_shutdown {
                        owner.seal();
                    }
                    log::info!("[shutdown] Triggering background task shutdown (async, not awaited)...");
                    shutdown_for_exit.trigger();
                    let scanner_shutdown = match snapshot_scanner_shutdown {
                        Some(owner) => tauri::async_runtime::block_on(owner.seal_and_drain_until(
                            tokio::time::Instant::now()
                                + std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                        )),
                        None => phone::terminal_snapshot::SnapshotScannerDrainResult {
                            terminal: true,
                            ..Default::default()
                        },
                    };
                    log::info!(
                        "[shutdown] terminal snapshot scanner drained joined={} aborted={} \
                            terminal={}",
                        scanner_shutdown.joined,
                        scanner_shutdown.aborted,
                        scanner_shutdown.terminal
                    );

                    let context_alert_monitor = app_handle
                        .try_state::<Arc<crate::session::context_alerts::ContextAlertMonitor>>()
                        .map(|monitor| Arc::clone(monitor.inner()));
                    if let Some(monitor) = context_alert_monitor.as_ref() {
                        monitor.request_close();
                    }

                    let selection_shutdown = tauri::async_runtime::block_on(
                        selection_coordinator_for_exit.close_and_join(),
                    );
                    log::info!("[shutdown] selection coordinator joined before global cleanup");

                    // Selection shutdown seals and accounts for session preparation before
                    // the alert actor consumes its sole join handle.
                    if let Some(monitor) = context_alert_monitor {
                        if let Err(error) = tauri::async_runtime::block_on(monitor.close_and_join()) {
                            log::warn!("[shutdown] context alert monitor join failed: {}", error);
                        }
                    }

                    // #632 A - kill every agent's Job Object: atomically terminates
                    // each jobbed session's whole descendant tree via the job handle.
                    // This is the durable, orphan-free guarantee for jobbed sessions.
                    let pty_mgr = app_handle.state::<Arc<Mutex<PtyManager>>>();
                    let pty_lock_budget =
                        std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS);
                    let container_backend = {
                        let deadline = std::time::Instant::now() + pty_lock_budget;
                        loop {
                            match pty_mgr.try_lock() {
                                Ok(guard) => break Some(guard.container_backend()),
                                Err(std::sync::TryLockError::Poisoned(error)) => {
                                    break Some(error.into_inner().container_backend());
                                }
                                Err(std::sync::TryLockError::WouldBlock)
                                    if std::time::Instant::now() < deadline =>
                                {
                                    std::thread::sleep(std::time::Duration::from_millis(2));
                                }
                                Err(std::sync::TryLockError::WouldBlock) => break None,
                            }
                        }
                    };
                    let mut container_shutdown = match container_backend {
                        Some(container_backend) => container_backend
                            .stop_all_started_containers_blocking(pty_lock_budget),
                        None => {
                            log::error!(
                                "[shutdown] PTY owner lock reached the global cleanup deadline \
                                    before container ownership transfer"
                            );
                            crate::pty::container_backend::ContainerShutdownReport {
                                terminal: false,
                                retained: vec![
                                    "reason=global-pty-owner state=retained".to_string(),
                                ],
                            }
                        }
                    };
                    let jobs = {
                        let deadline = std::time::Instant::now() + pty_lock_budget;
                        loop {
                            match pty_mgr.try_lock() {
                                Ok(guard) => break Some(guard.kill_all_jobs()),
                                Err(std::sync::TryLockError::Poisoned(error)) => {
                                    break Some(error.into_inner().kill_all_jobs());
                                }
                                Err(std::sync::TryLockError::WouldBlock)
                                    if std::time::Instant::now() < deadline =>
                                {
                                    std::thread::sleep(std::time::Duration::from_millis(2));
                                }
                                Err(std::sync::TryLockError::WouldBlock) => break None,
                            }
                        }
                    };
                    let (jobs_killed, jobless_sessions) = match jobs {
                        Some(counts) => counts,
                        None => {
                            log::error!(
                                "[shutdown] PTY owner lock reached the job cleanup deadline \
                                    state=retained"
                            );
                            container_shutdown.terminal = false;
                            container_shutdown
                                .retained
                                .push("reason=global-job-owner state=retained".to_string());
                            (0, 0)
                        }
                    };
                    log::info!(
                        "[shutdown] terminated {jobs_killed} agent job object(s); \
                            {jobless_sessions} session(s) had no job"
                    );

                    // #632 B2+B4 - run the identity reaper for accounting and as the
                    // backstop for job-less sessions, TIME-BOXED. Fresh-only targets
                    // (B2a) make it near-instant for jobbed sessions (their live tree is
                    // already dead). Abandoning it on timeout is safe for jobbed
                    // sessions; the job-less warning below covers the MED-2 residual.
                    let rm_for_cleanup = resource_monitor_for_exit.clone();
                    let cleanup = crate::shutdown::run_time_boxed(
                        std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                        move || {
                            rm_for_cleanup.kill_all_owned_groups(
                                resource_monitor::ResourceKillReason::AppShutdown,
                            )
                        },
                    );
                    match &cleanup {
                        Ok(results) => {
                            for result in results {
                                if result.quarantined {
                                    log::warn!(
                                        "[shutdown] resource group {} quarantined during \
                                            cleanup: {}",
                                        result.session_id,
                                        result.message
                                    );
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => log::warn!(
                            "[shutdown] resource cleanup exceeded \
                                {SHUTDOWN_CLEANUP_BUDGET_SECS}s budget; proceeding to exit (job \
                                objects already terminated jobbed trees)"
                        ),
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => log::error!(
                            "[shutdown] resource cleanup thread panicked before completing; \
                                jobbed trees were still terminated by the job kill"
                        ),
                    }
                    // #632 MED-2 - job-less sessions are reaper-only; if the reaper did
                    // not finish, their trees may be orphaned. Make it visible.
                    if cleanup.is_err() && jobless_sessions > 0 {
                        log::warn!(
                            "[shutdown] {jobless_sessions} session(s) had no Job Object and the \
                                reaper did not finish in budget; their process trees may be \
                                orphaned"
                        );
                    }

                    if shutdown_persistence_allowed(
                        selection_shutdown.persistence_safe,
                        container_shutdown.terminal,
                    ) {
                        log::info!("[shutdown] Persisting session state...");
                        let mgr_clone = session_mgr_for_exit.clone();
                        tauri::async_runtime::block_on(async move {
                            let mgr = mgr_clone.read().await;
                            sessions_persistence::persist_current_state(&mgr).await;
                        });
                        log::info!("[shutdown] Session state persisted, process exiting");
                    } else {
                        let retained = combined_shutdown_retained_diagnostics_with_scanner(
                            scanner_shutdown.retained,
                            selection_shutdown.retained,
                            container_shutdown.retained,
                        );
                        log::error!(
                            "[shutdown] final session persistence skipped because cleanup \
                                ownership is retained work=[{}]",
                            retained.join(", ")
                        );
                    }

                    // #552 flush the coordinator badge / auto-closed store on clean
                    // exit (the 60s tick can leave up to one tick of recency
                    // unpersisted). Sync snapshot + save; best-effort on poison.
                    {
                        let snap = coordinator_clocks_for_exit
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .snapshot();
                        if let Err(e) = crate::config::coordinator_clocks::save_map(&snap) {
                            log::warn!("[coordinator-clocks] exit flush failed: {}", e);
                        }
                    }

                    // Issue #231 + grinch G-LOW (#246): remove daemon.pid AFTER
                    // persist_current_state so a concurrent CLI invocation never
                    // observes NoPidFile while sessions.json is being rewritten.
                    // Still runs before process exit — subsequent CLI invocations
                    // see NoPidFile (not StalePidFile) once we return.
                    crate::config::daemon_pid::remove_pid_file();
                    ui_automation_state_for_exit.cleanup_session_file();
                }
                _ => {}
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests;
