pub mod cli;
pub mod commands;
pub mod config;
pub mod errors;
pub mod logging;
pub mod network;
pub mod phone;
pub mod pty;
pub mod session;
pub mod shutdown;
pub mod telegram;
pub mod testability;
pub mod voice;
pub mod web;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use commands::ac_discovery::DiscoveryBranchWatcher;
use config::sessions_persistence;
use config::settings::SettingsState;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use session::manager::SessionManager;
use shutdown::ShutdownSignal;
use tauri::Manager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use voice::tracker::{VoiceTracker, VoiceTrackingState};
use web::auth::WebAccessToken;
use web::broadcast::WsBroadcaster;

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

/// Handle to the running web server task, allowing stop control.
pub type WebServerHandle = Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>;

/// Issue #120 — serializes in-process writers of `.claude/settings.local.json`.
///
/// Acquired by `commands::config::sweep_rtk_hook`, the startup auto-disable
/// and active-recovery sweeps in `setup`, and every in-process call site that
/// invokes `ensure_claude_md_excludes` + `ensure_rtk_pretool_hook` (see plan
/// §7.5). Cross-process races (CLI / second AC instance) are documented and
/// out of scope for #120.
pub type RtkSweepLockState = Arc<tokio::sync::Mutex<()>>;

/// Issue #120 §18 — cached boot-time RTK startup mode.
///
/// Set ONCE by the setup task in `run()` after computing the mode and
/// BEFORE running any side effects. Read by
/// `commands::config::get_rtk_startup_status` so the getter returns the
/// boot decision instead of recomputing from post-side-effect state
/// (which would mismatch the listener for the `auto-disabled` mode).
pub type RtkStartupModeState = Arc<std::sync::OnceLock<String>>;

/// Floating spec/Mermaid board document state.
pub type SpecBoardState = Arc<tokio::sync::RwLock<commands::spec_board::SpecBoardManager>>;

/// Master token generated at app startup. Allows bypassing team validation (can_reach).
/// Persisted to `master-token.txt` in config_dir for CLI use. Regenerated on each app startup. See #34.
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

/// Decide whether a persisted session should be restored with a live PTY
/// at app startup, or deferred (created as a dormant `Exited(0)` record).
///
/// Inputs:
///   - `setting_on`: value of `AppSettings::restore_coordinator_wake_state`.
///   - `is_coord`: whether the agent FQN derived from `ps.working_directory`
///     is a coordinator of any discovered team.
///   - `persisted_status`: `PersistedSession::status` as snapshotted at the
///     last app shutdown. `None` means the snapshot was taken by an older
///     binary that did not record status. Treat `None` as **awake** for
///     forward-compat — better to wake a coord the user expected to be
///     awake than silently leave it dormant on first launch after upgrade.
///
/// Returns true ⇒ restore with PTY; false ⇒ defer (dormant).
pub(crate) fn should_wake_on_restore(
    setting_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    if !setting_on {
        return false; // Setting OFF: defer everything.
    }
    if !is_coord {
        return false; // Non-coord: always deferred under the new policy.
    }
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false, // asleep at shutdown
        Some(_) | None => true, // awake at shutdown (or unknown → fail-open)
    }
}

pub(crate) fn should_wake_root_agent_on_restore(
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false,
        Some(crate::session::session::SessionStatus::Active)
        | Some(crate::session::session::SessionStatus::Running)
        | Some(crate::session::session::SessionStatus::Idle)
        | None => true,
    }
}

pub(crate) fn should_auto_create_root_agent_on_first_restore(
    settings: &crate::config::settings::AppSettings,
    last_coding_agent: Option<&str>,
) -> bool {
    commands::session::resolve_root_agent_command(settings, None, last_coding_agent).is_ok()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
) {
    // Same backend the CLI path now installs in `main.rs` — see `logging.rs`
    // for the rationale. Idempotent, so a hypothetical second call (or the
    // CLI path having already run in this process) is a no-op.
    crate::logging::init_logger();

    // Generate master token — printed to stdout and persisted to master-token.txt for CLI use
    let master_token = MasterToken::new(uuid::Uuid::new_v4().to_string());

    // Create instance-private outbox directory and clean up stale ones
    let config_dir = config::config_dir().expect("Cannot determine home directory");
    let instances_dir = config_dir.join("instances");

    // Clean up old instance dirs (from previous runs)
    if let Ok(entries) = std::fs::read_dir(&instances_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
        log::info!("[app-outbox] Cleaned stale instance directories");
    }

    let instance_id = uuid::Uuid::new_v4().to_string();
    let app_outbox_path = instances_dir.join(&instance_id).join("outbox");
    std::fs::create_dir_all(&app_outbox_path).expect("Failed to create app outbox directory");
    let app_outbox = AppOutbox::new(app_outbox_path.to_string_lossy().to_string());
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

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let shutdown_signal = ShutdownSignal::new();

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));

    // Idle detector: emits session_idle / session_busy events.
    // Callbacks run on native threads (watcher + PTY read loop).
    // AppHandle.emit() is sync and thread-safe, so no tokio needed.
    // AppHandle is set in setup() via OnceLock; callbacks no-op until then.
    let app_handle_lock: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let handle_for_idle = Arc::clone(&app_handle_lock);
    let handle_for_busy = Arc::clone(&app_handle_lock);
    let idle_detector = IdleDetector::new(
        move |id| {
            log::info!("[idle] >>> EMIT session_idle for {}", &id.to_string()[..8]);
            if let Some(app) = handle_for_idle.get() {
                let _ = tauri::Emitter::emit(
                    app,
                    "session_idle",
                    serde_json::json!({ "id": id.to_string() }),
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_idle(id).await;
                    crate::config::sessions_persistence::persist_current_state(&mgr).await;
                });
            }
        },
        move |id| {
            log::info!("[idle] >>> EMIT session_busy for {}", &id.to_string()[..8]);
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
                    crate::config::sessions_persistence::persist_current_state(&mgr).await;
                });
            }
        },
    );
    idle_detector.start(shutdown_signal.clone());

    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_discovery = Arc::clone(&session_mgr);
    let session_mgr_for_web = Arc::clone(&session_mgr);
    let session_mgr_for_exit = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);
    let broadcaster_for_pty = broadcaster.clone();
    let broadcaster_for_web = broadcaster.clone();
    let web_token_for_server = Arc::clone(&web_access_token);

    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(output_senders),
    ));

    let settings: SettingsState =
        Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));
    let settings_for_web = Arc::clone(&settings);
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        commands::spec_board::SpecBoardManager::new(),
    ));

    // Issue #120 — RTK sweep mutex. Acquired by every in-process writer of
    // `.claude/settings.local.json`. See plan §7.5 for the design.
    let rtk_sweep_lock: RtkSweepLockState = Arc::new(tokio::sync::Mutex::new(()));
    let rtk_sweep_lock_for_setup = Arc::clone(&rtk_sweep_lock);

    // Issue #120 §18 — cached boot-time RTK startup mode. Set ONCE by the
    // setup task before running side effects; read by
    // `get_rtk_startup_status` so the late-mounting banner sees the SAME
    // mode the listener received.
    let rtk_startup_mode: RtkStartupModeState = Arc::new(std::sync::OnceLock::new());
    let rtk_startup_mode_for_setup = Arc::clone(&rtk_startup_mode);

    let shutdown_for_setup = shutdown_signal.clone();
    let shutdown_for_exit = shutdown_signal.clone();
    let tg_mgr_for_exit = tg_mgr.clone();
    let ui_automation_state_for_setup = ui_automation_state.clone();
    let ui_automation_state_for_exit = ui_automation_state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(tg_mgr)
        .manage(network::OutboundNetwork::new().expect("failed to build shared network clients"))
        .manage(voice_tracking)
        .manage(settings)
        .manage(detached_sessions.clone())
        .manage(spec_board_state.clone())
        .manage(web_access_token.clone())
        .manage(broadcaster.clone())
        .manage(WebServerHandle::default())
        .manage(rtk_sweep_lock)
        .manage(rtk_startup_mode)
        .manage(ui_automation_state)
        .manage(shutdown_signal)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            // #264 — spawn the background task that emits `error_log_event`
            // pings to the UI when ERROR entries are captured. The task runs
            // OUTSIDE the env_logger format closure (see §3.7 / B1). Entries
            // logged before this point stay buffered; the frontend's first
            // `drain_error_logs` call collects them.
            crate::logging::spawn_error_emit_task(app.handle().clone());

            // #271 — seed `<config_dir>/agent-templates/` + README on startup.
            crate::commands::role_templates::ensure_default_templates_dir_at_config();

            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start(shutdown_for_setup.clone());
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            discovery_branch_watcher.start(shutdown_for_setup.clone());
            app.manage(discovery_branch_watcher);

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                git_watcher,
                Some(broadcaster_for_pty),
            )));
            app.manage(pty_mgr.clone());

            // Start web server if enabled in settings
            {
                let web_settings = config::settings::load_settings();
                if web_settings.web_server_enabled {
                    let bind = web_settings.web_server_bind.clone();
                    let port = web_settings.web_server_port;
                    println!("[web-token] Remote URL: http://{}:{}/?window=main&remoteToken={}", bind, port, web_access_token.value());

                    let join_handle = web::start_server(
                        bind,
                        port,
                        web_token_for_server,
                        session_mgr_for_web,
                        pty_mgr.clone(),
                        settings_for_web,
                        broadcaster_for_web,
                        app.handle().clone(),
                        shutdown_for_setup.clone(),
                    );

                    let ws_handle = app.state::<WebServerHandle>();
                    *ws_handle.lock().unwrap() = Some(join_handle);
                }
            }

            // Issue #120 / #426 — RTK startup detection. Probes PATH for `rtk`,
            // then maps (rtk_present, inject_rtk_hook, rtk_prompt_dismissed,
            // inform_when_rtk_installed) to a mode via
            // `crate::config::settings::compute_rtk_startup_mode`:
            //   - prompt-enable: rtk found AND inject_rtk_hook=false AND
            //       rtk_prompt_dismissed=false AND inform_when_rtk_installed=true.
            //       The banner is opt-in (issue #426): default-false inform means
            //       no banner unless the user enabled it in Settings.
            //   - active: rtk found AND inject_rtk_hook=true. Emits mode="active"
            //       plus an active-recovery ON-sweep (idempotent).
            //   - auto-disabled: rtk missing AND inject_rtk_hook=true. Persists
            //       inject_rtk_hook=false (write lock held through save: grinch
            //       H4 + N1); sweeps with enabled=false (RtkSweepLock held: grinch
            //       M8); emits mode="auto-disabled".
            //   - silent: everything else (frontend treats as no-op).
            // Detached so the rest of setup is not blocked by disk I/O.
            {
                let app_handle_for_rtk = app.handle().clone();
                let sweep_lock = Arc::clone(&rtk_sweep_lock_for_setup);
                let mode_cache = Arc::clone(&rtk_startup_mode_for_setup);
                tauri::async_runtime::spawn(async move {
                    use crate::config::claude_settings::{
                        enumerate_managed_agent_dirs, ensure_rtk_pretool_hook,
                    };

                    let rtk_present = which::which("rtk").is_ok();

                    let settings_state = app_handle_for_rtk
                        .state::<crate::config::settings::SettingsState>();

                    let (inject_enabled, prompt_dismissed, inform_when_installed) = {
                        let s = settings_state.read().await;
                        (
                            s.inject_rtk_hook,
                            s.rtk_prompt_dismissed,
                            s.inform_when_rtk_installed,
                        )
                    };

                    let mode: &'static str = crate::config::settings::compute_rtk_startup_mode(
                        rtk_present,
                        inject_enabled,
                        prompt_dismissed,
                        inform_when_installed,
                    );

                    // §18 — cache the boot decision BEFORE running side effects
                    // so a late-mounting banner sees the SAME mode the listener
                    // receives (auto-disabled side-effects mutate inject_rtk_hook,
                    // breaking any naïve recompute path). `set` returns Err if
                    // already set; ignore (idempotent for repeated boots).
                    let _ = mode_cache.set(mode.to_string());

                    if mode == "auto-disabled" {
                        // H4 + N1 fix: hold the SettingsState write lock through
                        // save_settings so a concurrent update_settings cannot
                        // land between our in-memory flip and the disk persist.
                        // The lock is released explicitly via drop(s) AFTER the
                        // save returns.
                        let mut s = settings_state.write().await;
                        s.inject_rtk_hook = false;
                        let snapshot = s.clone();
                        if let Err(e) = crate::config::settings::save_settings(&snapshot) {
                            log::warn!("[rtk-startup] Failed to persist auto-disable: {}", e);
                        }
                        let project_paths = snapshot.project_paths.clone();
                        drop(s); // explicit; lock released AFTER the disk write

                        // M8 fix: hold RtkSweepLock through the OFF-sweep loop.
                        let _guard = sweep_lock.lock().await;
                        for dir in enumerate_managed_agent_dirs(&project_paths) {
                            if let Err(e) = ensure_rtk_pretool_hook(&dir, false) {
                                log::warn!(
                                    "[rtk-startup] auto-disable sweep failed for {}: {}",
                                    dir.display(),
                                    e
                                );
                            }
                        }
                    } else if mode == "active" {
                        // M8 fix: hold RtkSweepLock through the ON-sweep loop.
                        let project_paths = {
                            let s = settings_state.read().await;
                            s.project_paths.clone()
                        };
                        let _guard = sweep_lock.lock().await;
                        for dir in enumerate_managed_agent_dirs(&project_paths) {
                            if let Err(e) = ensure_rtk_pretool_hook(&dir, true) {
                                log::warn!(
                                    "[rtk-startup] active recovery sweep failed for {}: {}",
                                    dir.display(),
                                    e
                                );
                            }
                        }
                    }

                    let _ = tauri::Emitter::emit(
                        &app_handle_for_rtk,
                        "rtk_startup_status",
                        serde_json::json!({ "mode": mode }),
                    );

                    log::info!(
                        "[rtk-startup] mode={} rtkPresent={} injectEnabled={} promptDismissed={} informWhenInstalled={}",
                        mode,
                        rtk_present,
                        inject_enabled,
                        prompt_dismissed,
                        inform_when_installed
                    );
                });
            }

            if let Err(e) = crate::config::root_agent::ensure_root_agent_dir() {
                log::error!("[root-agent] Failed to provision root agent directory: {}", e);
            }

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
            let persisted = sessions_persistence::load_sessions_purging_outside_project_paths(
                &restore_settings_snapshot.project_paths,
            );
            let restore_flag = app
                .state::<Arc<RestoreInProgress>>()
                .inner()
                .clone();
            restore_flag
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);

            // Start the mailbox poller for inter-agent message delivery
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.handle().clone(), shutdown_for_setup.clone());

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

            /// Check if at least 50px of a window (physical coords) is visible on any monitor
            fn is_visible_on_monitors(
                geo: &config::settings::WindowGeometry,
                monitors: &[(f64, f64, f64, f64, f64)],
            ) -> bool {
                if monitors.is_empty() {
                    return true; // Can't validate, assume OK
                }
                let margin = 50.0;
                monitors.iter().any(|(mx, my, mx2, my2, _)| {
                    geo.x + geo.width > mx + margin
                        && geo.x < mx2 - margin
                        && geo.y + geo.height > my + margin
                        && geo.y < my2 - margin
                })
            }

            /// Convert saved geometry (physical pixels) to logical pixels for the builder.
            /// Finds which monitor the geometry center falls on and divides by that scale.
            fn physical_to_logical(
                geo: &config::settings::WindowGeometry,
                monitors: &[(f64, f64, f64, f64, f64)],
            ) -> config::settings::WindowGeometry {
                let cx = geo.x + geo.width / 2.0;
                let cy = geo.y + geo.height / 2.0;
                let scale = monitors
                    .iter()
                    .find(|(mx, my, mx2, my2, _)| cx >= *mx && cx < *mx2 && cy >= *my && cy < *my2)
                    .map(|(_, _, _, _, s)| *s)
                    .unwrap_or(1.0);
                config::settings::WindowGeometry {
                    x: geo.x / scale,
                    y: geo.y / scale,
                    width: geo.width / scale,
                    height: geo.height / scale,
                }
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
            let default_w = screen_w.min(1400.0);
            let default_h = screen_h.min(900.0);
            let default_main = config::settings::WindowGeometry {
                x: primary_x + (screen_w - default_w) / 2.0,
                y: primary_y + (screen_h - default_h) / 2.0,
                width: default_w,
                height: default_h,
            };

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
                                    "[test-window] MonitorFromRect returned null for requested rect ({}, {}) {}x{}",
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
                                        "[test-window] GetMonitorInfoW failed for requested rect ({}, {}) {}x{}",
                                        x,
                                        y,
                                        width,
                                        height
                                    );
                                } else {
                                    log::info!(
                                        "[test-window] selected monitor rect=({}, {}) {}x{} work=({}, {}) {}x{} for requested rect ({}, {}) {}x{} maximized={}",
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

            // Resolve main geometry: saved (physical) -> validate -> convert to logical -> fallback.
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
                    "[test-window] requested physical ({}, {}) {}x{} maximized={} -> logical ({}, {}) {}x{}",
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
                            "[window-setup] main: saved physical ({}, {}) {}x{} -> logical ({}, {}) {}x{}",
                            geo.x, geo.y, geo.width, geo.height,
                            logical.x, logical.y, logical.width, logical.height
                        );
                        logical
                    }
                    Some(geo) => {
                        log::warn!(
                            "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, falling back to centered default",
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
            .zoom_hotkeys_enabled(true)
            .inner_size(main_geo.width, main_geo.height)
            .position(main_geo.x, main_geo.y)
            .build()?;

            if let Some(test_geo) = &test_window_placement {
                let native_handled = apply_test_window_placement(&main_win, test_geo);
                if test_geo.maximized && !native_handled {
                    if let Err(e) = main_win.maximize() {
                        log::warn!("[test-window] failed to maximize main window: {}", e);
                    }
                }
                log_main_window_info(&main_win);
            }

            if saved_settings.main_always_on_top {
                let _ = main_win.set_always_on_top(true);
            }

            ui_automation_state_for_setup.start(app.handle().clone(), shutdown_for_setup.clone());

            // Suppress unused variable warning
            let _ = &main_win;

            // Restore sessions from last run
            //
            // §224 G-IMPL-1 — `persisted` and `restore_flag` are hoisted above
            // mailbox_poller.start() (see comment block there). `persisted` is
            // reused here; the flag is already TRUE when we enter this block.
            {
                use tauri::Manager;
                let session_mgr_clone = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>().inner().clone();
                let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                let tg_mgr_clone = app.state::<TelegramBridgeState>().inner().clone();
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
                    "[restore] {} teams discovered across {} project paths; setting_on={}; evaluating {} persisted sessions",
                    teams.len(),
                    settings_snapshot.project_paths.len(),
                    setting_on,
                    persisted.len()
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

                tauri::async_runtime::spawn(async move {
                    struct RestoreGuard(Arc<RestoreInProgress>);
                    impl Drop for RestoreGuard {
                        fn drop(&mut self) {
                            self.0
                                 .0
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    let _restore_guard = RestoreGuard(restore_flag_for_task);

                    let mut active_id = None;
                    let mut failed_recoverable: Vec<sessions_persistence::PersistedSession> = Vec::new();

                    // #248 Grinch Z5 — count outcomes for the end-of-restore summary line.
                    let mut n_woken: usize = 0;
                    let mut n_deferred: usize = 0;

                    let root_agent_path = match crate::config::root_agent::ensure_root_agent_dir() {
                        Ok(path) => Some(path),
                        Err(e) => {
                            log::error!("[root-agent] Failed to provision root agent during restore: {}", e);
                            None
                        }
                    };
                    let root_ps = persisted
                        .iter()
                        .find(|ps| {
                            ps.is_root_agent
                                || crate::config::root_agent::is_root_agent_path(
                                    &ps.working_directory,
                                )
                        })
                        .cloned();

                    if let Some(root_path) = root_agent_path.clone() {
                        match root_ps.as_ref() {
                            None => {
                                let last_coding_agent =
                                    crate::config::root_agent::read_last_coding_agent(&root_path);
                                let should_auto_create = {
                                    let cfg = settings_state_clone.read().await;
                                    should_auto_create_root_agent_on_first_restore(
                                        &cfg,
                                        last_coding_agent.as_deref(),
                                    )
                                };

                                if should_auto_create {
                                    match commands::session::create_root_agent_inner(
                                        &app_handle,
                                        &session_mgr_clone,
                                        &pty_mgr_clone,
                                        &tg_mgr_clone,
                                        &settings_state_clone,
                                        None,
                                        None,
                                        true,
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
                                        "[root-agent] Skipping startup auto-create: no resolvable coding agent is configured"
                                    );
                                }
                            }
                            Some(ps)
                                if should_wake_root_agent_on_restore(ps.status.as_ref()) =>
                            {
                                let _root_guard =
                                    commands::session::root_agent_session_lock().lock().await;
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
                                            if let Err(e) =
                                                commands::session::force_destroy_session_inner(
                                                    &app_handle,
                                                    uuid,
                                                )
                                                .await
                                            {
                                                log::warn!(
                                                    "[root-agent] Failed to force-destroy stale dormant root during restore: {}",
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
                                            commands::session::attach_persisted_telegram_if_configured(
                                                &app_handle,
                                                uuid,
                                                ps.telegram_bot_id.as_deref(),
                                            )
                                            .await;
                                            if let Some(ref prompt) = ps.last_prompt {
                                                let mgr = session_mgr_clone.read().await;
                                                mgr.set_last_prompt(uuid, prompt.clone()).await;
                                            }
                                        }
                                        should_create = false;
                                    }
                                }
                                if should_create {
                                    let mut rebuild_failed = false;
                                    let resolved_spawn = if let Some(aid) = ps.agent_id.as_deref() {
                                        match commands::session::build_configured_agent_spawn_for_cwd(
                                            &settings_snapshot,
                                            aid,
                                            &root_path,
                                            ps.requested_profile.as_deref(),
                                        ) {
                                            Ok(spawn) => spawn,
                                            Err(e) => {
                                                log::error!(
                                                    "[root-agent] Failed to rebuild configured agent command for restore '{}': {}",
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
                                    match commands::session::create_session_inner(
                                        &app_handle,
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
                                    )
                                    .await
                                    {
                                        Ok(info) => {
                                            if ps.was_active {
                                                active_id = Some(info.id.clone());
                                            }
                                            n_woken += 1;
                                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                commands::session::attach_persisted_telegram_if_configured(
                                                    &app_handle,
                                                    uuid,
                                                    ps.telegram_bot_id.as_deref(),
                                                )
                                                .await;
                                            }

                                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                if let Some(ref prompt) = ps.last_prompt {
                                                    let mgr = session_mgr_clone.read().await;
                                                    mgr.set_last_prompt(uuid, prompt.clone()).await;
                                                }
                                            }

                                            if ps.was_detached {
                                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                    {
                                                        let mgr = session_mgr_clone.read().await;
                                                        mgr.set_was_detached(uuid, true).await;
                                                        if let Some(ref geo) = ps.detached_geometry
                                                        {
                                                            mgr.set_detached_geometry(
                                                                uuid,
                                                                geo.clone(),
                                                            )
                                                            .await;
                                                        }
                                                    }

                                                    let detached_state =
                                                        app_handle.state::<DetachedSessionsState>();
                                                    let detached_result =
                                                        commands::window::detach_terminal_inner(
                                                            &app_handle,
                                                            &session_mgr_clone,
                                                            detached_state.inner(),
                                                            &info.id,
                                                            ps.detached_geometry.clone(),
                                                            true,
                                                        )
                                                        .await;
                                                    if let Err(e) = detached_result {
                                                        log::warn!(
                                                            "[restore] detach_terminal_inner failed for root agent '{}': {} — session stays live (attached)",
                                                            ps.name,
                                                            e
                                                        );
                                                    } else {
                                                        let mgr = session_mgr_clone.read().await;
                                                        mgr.clear_active_if(uuid).await;
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
                                let _root_guard =
                                    commands::session::root_agent_session_lock().lock().await;
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
                                        if let Some(ref prompt) = ps.last_prompt {
                                            mgr.set_last_prompt(uuid, prompt.clone()).await;
                                        }
                                    }
                                    if ps.was_active {
                                        active_id = Some(existing.id.clone());
                                    }
                                    n_deferred += 1;
                                } else {
                                    let mgr = session_mgr_clone.read().await;
                                    match mgr
                                        .create_session(
                                            ps.shell.clone(),
                                            ps.shell_args.clone(),
                                            root_path,
                                            ps.agent_id.clone(),
                                            ps.agent_label.clone(),
                                            ps.git_repos.clone(),
                                            false,
                                        )
                                        .await
                                    {
                                        Ok(session) => {
                                            mgr.rename_session(session.id, ps.name.clone())
                                                .await
                                                .ok();
                                            mgr.set_is_root_agent(session.id, true).await;
                                            if ps.was_detached {
                                                mgr.set_was_detached(session.id, true).await;
                                            }
                                            if let Some(ref geo) = ps.detached_geometry {
                                                mgr.set_detached_geometry(session.id, geo.clone())
                                                    .await;
                                            }
                                            if let Some(ref prompt) = ps.last_prompt {
                                                mgr.set_last_prompt(session.id, prompt.clone()).await;
                                            }
                                            commands::session::preserve_deferred_telegram_intent_if_valid(
                                                &mgr,
                                                &settings_state_clone,
                                                session.id,
                                                &ps.name,
                                                ps.telegram_bot_id.as_deref(),
                                            )
                                            .await;
                                            mgr.mark_exited(session.id, 0).await;
                                            mgr.clear_active_if(session.id).await;
                                            if let Some(updated) =
                                                mgr.get_session(session.id).await
                                            {
                                                let info =
                                                    crate::session::session::SessionInfo::from(
                                                        &updated,
                                                    );
                                                let _ = tauri::Emitter::emit(
                                                    &app_handle,
                                                    "session_created",
                                                    info,
                                                );
                                            }
                                            if ps.was_active {
                                                active_id = Some(session.id.to_string());
                                            }
                                            n_deferred += 1;
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "[root-agent] Failed to create dormant root session '{}': {}",
                                                ps.name,
                                                e
                                            );
                                            failed_recoverable.push(ps.clone());
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(ps) = root_ps.as_ref() {
                        failed_recoverable.push(ps.clone());
                    }

                    for ps in &persisted {
                        if ps.is_root_agent
                            || crate::config::root_agent::is_root_agent_path(
                                &ps.working_directory,
                            )
                        {
                            continue;
                        }

                        // Skip sessions whose CWD no longer exists (permanent failure)
                        if !std::path::Path::new(&ps.working_directory).exists() {
                            log::warn!("Skipping restore of '{}': CWD '{}' no longer exists", ps.name, ps.working_directory);
                            continue;
                        }

                        // #248 — decide wake vs defer for this session.
                        // §DR2: use `agent_fqn_from_path` so WG replicas get project-precise
                        // team membership and coordinator checks. Strict `is_coordinator`
                        // (§AR2-strict) requires the FQN to avoid cross-project flag leaks.
                        let agent_name = crate::config::teams::agent_fqn_from_path(&ps.working_directory);
                        let is_coord = crate::config::teams::is_any_coordinator(&agent_name, &teams);
                        let wake = should_wake_on_restore(setting_on, is_coord, ps.status.as_ref());

                        if !wake {
                            // Defer: create a dormant Session record (no PTY, status = Exited(0)).
                            let mgr = session_mgr_clone.read().await;
                            match mgr.create_session(
                                ps.shell.clone(),
                                ps.shell_args.clone(),
                                ps.working_directory.clone(),
                                ps.agent_id.clone(),
                                ps.agent_label.clone(),
                                ps.git_repos.clone(),
                                is_coord, // #248 — pass live is_coord so dormant coords stay coords (Z7).
                            ).await {
                                Ok(session) => {
                                    mgr.rename_session(session.id, ps.name.clone()).await.ok();

                                    // Grinch Z2 — preserve detach state across the defer lifecycle.
                                    // Must be applied BEFORE mark_exited so the next snapshot_sessions
                                    // event (whether triggered by failed_recoverable, user action, or
                                    // shutdown) writes was_detached=true to disk. Without this, the
                                    // first persist after defer silently drops the user's detach intent
                                    // (manager.rs defaults was_detached to false on create_session).
                                    if ps.was_detached {
                                        mgr.set_was_detached(session.id, true).await;
                                    }
                                    if let Some(ref geo) = ps.detached_geometry {
                                        mgr.set_detached_geometry(session.id, geo.clone()).await;
                                    }
                                    if let Some(ref prompt) = ps.last_prompt {
                                        mgr.set_last_prompt(session.id, prompt.clone()).await;
                                    }
                                    commands::session::preserve_deferred_telegram_intent_if_valid(
                                        &mgr,
                                        &settings_state_clone,
                                        session.id,
                                        &ps.name,
                                        ps.telegram_bot_id.as_deref(),
                                    )
                                    .await;

                                    mgr.mark_exited(session.id, 0).await;
                                    mgr.clear_active_if(session.id).await;
                                    // Read updated session so emitted event reflects Exited status
                                    if let Some(updated) = mgr.get_session(session.id).await {
                                        let info = crate::session::session::SessionInfo::from(&updated);
                                        let _ = tauri::Emitter::emit(&app_handle, "session_created", info);
                                    }
                                    // Grinch Z5 — debug, not info: under the new default every
                                    // persisted session lands here, and an info-level line per
                                    // session creates a "mass defer" wall in startup logs that
                                    // looks like an alarm. The end-of-loop info summary below
                                    // carries the load-bearing signal.
                                    log::debug!(
                                        "Deferred session '{}' on startup (agent: {}, is_coord: {}, setting: {}, persisted_status: {:?}, was_detached: {})",
                                        ps.name, agent_name, is_coord, setting_on, ps.status, ps.was_detached
                                    );
                                    n_deferred += 1;
                                    // Preserve `was_active` for the post-loop active-switch:
                                    // a deferred session can still be the persisted-active one.
                                    // The post-loop branching (Fix A) ensures `set_active_only`
                                    // is used (not `switch_session`), so the dormant status
                                    // survives selection.
                                    if ps.was_active {
                                        active_id = Some(session.id.to_string());
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
                        let resolved_spawn = if let Some(aid) = ps.agent_id.as_deref() {
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

                        // Wake: full PTY restore via the existing create_session_inner call.
                        match commands::session::create_session_inner(
                            &app_handle,
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
                            false, // skip_auto_resume = false → restore path; allow `--continue`
                            resolved_spawn,
                        ).await {
                            Ok(info) => {
                                if ps.was_active {
                                    active_id = Some(info.id.clone());
                                }
                                n_woken += 1;
                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                    commands::session::attach_persisted_telegram_if_configured(
                                        &app_handle,
                                        uuid,
                                        ps.telegram_bot_id.as_deref(),
                                    )
                                    .await;
                                }

                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                    if let Some(ref prompt) = ps.last_prompt {
                                        let mgr = session_mgr_clone.read().await;
                                        mgr.set_last_prompt(uuid, prompt.clone()).await;
                                    }
                                }

                                // Phase 3 restore: reconstruct detach state for the live session.
                                // Deferred sessions (handled above with a `continue`) never reach
                                // this branch, so R.9's "skip detached-window spawn for deferred"
                                // guard is enforced structurally by this code path.
                                if ps.was_detached {
                                    if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                        // Sync Session::was_detached BEFORE calling
                                        // detach_terminal_inner. detach_terminal_inner also sets
                                        // it true idempotently, but setting up-front guards the
                                        // persistence-correctness invariant even if
                                        // detach_terminal_inner fails (WebView2 init error, etc.):
                                        // the session stays marked detached → next launch retries.
                                        {
                                            let mgr = session_mgr_clone.read().await;
                                            mgr.set_was_detached(uuid, true).await;
                                            if let Some(ref geo) = ps.detached_geometry {
                                                mgr.set_detached_geometry(uuid, geo.clone()).await;
                                            }
                                        }

                                        // PB-4: pass `&info.id` (the newly-live session's UUID),
                                        // NOT `ps.id` (the stale prior-run UUID).
                                        // skip_switch=true per R.10 / A3.3 so this per-session
                                        // detach does not race the post-loop active_id switch.
                                        let detached_state =
                                            app_handle.state::<DetachedSessionsState>();
                                        let detached_result = commands::window::detach_terminal_inner(
                                            &app_handle,
                                            &session_mgr_clone,
                                            detached_state.inner(),
                                            &info.id,
                                            ps.detached_geometry.clone(),
                                            true,
                                        )
                                        .await;
                                        if let Err(e) = detached_result {
                                            log::warn!(
                                                "[restore] detach_terminal_inner failed for '{}': {} — session stays live (attached)",
                                                ps.name,
                                                e
                                            );
                                        } else {
                                            let mgr = session_mgr_clone.read().await;
                                            mgr.clear_active_if(uuid).await;
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
                        n_woken, n_deferred, setting_on, persisted.len()
                    );

                    // Switch to the session that was active when the app closed. Plan §A2.2.G3
                    // filter: if the persisted-active session is now detached (respawned with
                    // `was_detached=true` above), do NOT switch main to it — main + detached
                    // would both render the same session. Walk the list for the first non-
                    // detached candidate; emit `session_switched` with null if none.
                    if let Some(id) = active_id {
                        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                            let is_detached = {
                                let detached = app_handle.state::<DetachedSessionsState>();
                                let set = detached.lock().unwrap();
                                set.contains(&uuid)
                            };
                            let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> =
                                session_mgr_clone.read().await;
                            if is_detached {
                                let sessions = mgr.list_sessions().await;
                                let fallback = {
                                    let detached = app_handle.state::<DetachedSessionsState>();
                                    let set = detached.lock().unwrap();
                                    sessions.iter().find_map(|s| {
                                        uuid::Uuid::parse_str(&s.id)
                                            .ok()
                                            .filter(|u| !set.contains(u))
                                    })
                                };
                                if let Some(fb) = fallback {
                                    let _ = mgr.switch_session(fb).await;
                                    let _ = tauri::Emitter::emit(
                                        &app_handle,
                                        "session_switched",
                                        serde_json::json!({ "id": fb.to_string() }),
                                    );
                                } else {
                                    mgr.clear_active().await;
                                    let _ = tauri::Emitter::emit(
                                        &app_handle,
                                        "session_switched",
                                        serde_json::json!({ "id": serde_json::Value::Null }),
                                    );
                                }
                            } else {
                                // #248 Fix A — preserve dormant status for the persisted-active
                                // session. Read the candidate's current status from the live
                                // manager (it was just set by either the defer arm or
                                // create_session_inner upstream).
                                let candidate_status =
                                    mgr.get_session(uuid).await.map(|s| s.status.clone());
                                let dormant = matches!(
                                    candidate_status,
                                    Some(crate::session::session::SessionStatus::Exited(_))
                                );

                                let result = if dormant {
                                    mgr.set_active_only(uuid).await
                                } else {
                                    mgr.switch_session(uuid).await
                                };
                                if let Err(e) = result {
                                    log::error!(
                                        "Failed to select persisted-active session {} (dormant={}): {}",
                                        id, dormant, e
                                    );
                                }

                                // `session_switched` payload is unchanged — sidebar uses it for
                                // the selection highlight; dot color is driven by `status` from
                                // the next `list_sessions` refresh (which now correctly returns
                                // `Exited(0)` for the dormant case thanks to set_active_only).
                                let _ = tauri::Emitter::emit(
                                    &app_handle,
                                    "session_switched",
                                    serde_json::json!({ "id": id }),
                                );
                            }
                            drop(mgr);
                        }
                    }

                    // Persist restored sessions + failed-but-recoverable entries
                    let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> = session_mgr_clone.read().await;
                    sessions_persistence::persist_merging_failed(&mgr, &failed_recoverable).await;

                    if !failed_recoverable.is_empty() {
                        log::warn!(
                            "Session restore: {} sessions failed (preserved for next attempt): {:?}",
                            failed_recoverable.len(),
                            failed_recoverable.iter().map(|s| &s.name).collect::<Vec<_>>()
                        );
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::create_session,
            commands::session::destroy_session,
            commands::session::restart_session,
            commands::session::switch_session,
            commands::session::rename_session,
            commands::session::set_last_prompt,
            commands::session::list_sessions,
            commands::session::get_active_session,
            commands::session::create_root_agent_session,
            commands::task::task_get_title,
            commands::task::task_set_title,
              commands::task::task_clean,
                        commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::config::get_settings,
            commands::config::update_settings,
            commands::config::save_settings_draft,
            commands::config::update_coding_agent_profiles,
            commands::config::update_coding_agent_env_settings,
            commands::config::set_agent_default_profile,
            commands::config::set_instance_profile_override,
            commands::config::resolve_coding_agent_profile,
            commands::config::set_inject_rtk_hook,
            commands::config::set_rtk_prompt_dismissed,
            commands::config::set_sounds_enabled,
            commands::config::set_theme_light,
            commands::config::sweep_rtk_hook,
            commands::config::get_rtk_startup_status,
            commands::repos::search_repos,
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
            commands::window::open_in_explorer,
            commands::window::open_guide_window,
            commands::window::open_spec_board_window,
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
            commands::phone::phone_send_message,
            commands::phone::phone_get_inbox,
            commands::phone::phone_list_agents,
            commands::phone::phone_ack_messages,
            commands::voice::voice_transcribe,
            commands::voice::voice_mark_recording,
            commands::voice::voice_had_typing,
            commands::config::save_debug_logs,
            commands::config::drain_error_logs,
            commands::config::open_web_remote,
            commands::config::start_web_server,
            commands::config::stop_web_server,
            commands::config::get_web_server_status,
            commands::config::get_instance_label,
            commands::config::fetch_home_markdown,
            commands::agent_creator::pick_folder,
            commands::agent_creator::create_agent_folder,
            commands::agent_creator::write_claude_settings_local,
            commands::ac_discovery::discover_ac_agents,
            commands::ac_discovery::check_project_path,
            commands::ac_discovery::create_ac_project,
            commands::ac_discovery::open_project,
            commands::ac_discovery::new_project,
            commands::ac_discovery::discover_project,
            commands::ac_discovery::get_replica_context_files,
            commands::ac_discovery::set_replica_context_files,
            commands::entity_creation::create_agent_matrix,
            commands::entity_creation::delete_agent_matrix,
            commands::entity_creation::list_all_agents,
            commands::entity_creation::create_team,
            commands::entity_creation::delete_team,
            commands::entity_creation::update_team,
            commands::entity_creation::get_team_config,
            commands::entity_creation::create_workgroup,
            commands::entity_creation::delete_workgroup,
            commands::entity_creation::sync_workgroup_repos,
            commands::role_templates::list_role_templates,
            commands::role_templates::get_agency_templates_status,
            commands::role_templates::update_agency_templates,
        ])
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
                    if label == "spec-board" {
                        let state = spec_board_state.clone();
                        tauri::async_runtime::spawn(async move {
                            commands::spec_board::spec_board_close_all(state).await;
                        });
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

                    log::info!("[shutdown] Triggering background task shutdown (async, not awaited)...");
                    shutdown_for_exit.trigger();

                    log::info!("[shutdown] Persisting session state...");
                    let mgr_clone = session_mgr_for_exit.clone();
                    tauri::async_runtime::block_on(async move {
                        let mgr = mgr_clone.read().await;
                        sessions_persistence::persist_current_state(&mgr).await;
                    });
                    log::info!("[shutdown] Session state persisted, process exiting");

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
}

#[cfg(test)]
mod tests {
    use super::{
        should_auto_create_root_agent_on_first_restore, should_wake_on_restore,
        should_wake_root_agent_on_restore,
    };
    use crate::config::settings::{AgentConfig, AppSettings};
    use crate::session::session::SessionStatus;

    fn settings_with_agent() -> AppSettings {
        AppSettings {
            agents: vec![AgentConfig {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#10b981".to_string(),
                git_pull_before: false,
                exclude_global_claude_md: false,
                envs: Vec::new(),
                isolate_codex_home: false,
            }],
            ..AppSettings::default()
        }
    }

    #[test]
    fn setting_off_always_defers() {
        assert!(!should_wake_on_restore(
            false,
            true,
            Some(&SessionStatus::Running)
        ));
        assert!(!should_wake_on_restore(false, false, None));
    }

    #[test]
    fn non_coord_always_defers_when_on() {
        assert!(!should_wake_on_restore(
            true,
            false,
            Some(&SessionStatus::Running)
        ));
    }

    #[test]
    fn coord_awake_at_shutdown_wakes_when_on() {
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Running)
        ));
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Idle)
        ));
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Active)
        ));
    }

    #[test]
    fn coord_asleep_at_shutdown_defers_when_on() {
        assert!(!should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Exited(0))
        ));
        assert!(!should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Exited(137))
        ));
    }

    #[test]
    fn coord_unknown_status_fails_open_when_on() {
        assert!(should_wake_on_restore(true, true, None));
    }

    #[test]
    fn root_agent_live_or_legacy_status_wakes() {
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Running
        )));
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Idle
        )));
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Active
        )));
        assert!(should_wake_root_agent_on_restore(None));
    }

    #[test]
    fn root_agent_exited_status_stays_dormant() {
        assert!(!should_wake_root_agent_on_restore(Some(
            &SessionStatus::Exited(0)
        )));
        assert!(!should_wake_root_agent_on_restore(Some(
            &SessionStatus::Exited(137)
        )));
    }

    #[test]
    fn first_restore_does_not_auto_create_root_agent_without_agents() {
        let settings = AppSettings::default();

        assert!(!should_auto_create_root_agent_on_first_restore(
            &settings, None
        ));
    }

    #[test]
    fn first_restore_auto_creates_root_agent_with_configured_agent() {
        let settings = settings_with_agent();

        assert!(should_auto_create_root_agent_on_first_restore(
            &settings, None
        ));
    }

    #[test]
    fn first_restore_auto_creates_root_agent_with_valid_last_coding_agent() {
        let settings = settings_with_agent();

        assert!(should_auto_create_root_agent_on_first_restore(
            &settings,
            Some("codex")
        ));
    }
}
