pub mod cli;
pub mod commands;
pub mod config;
pub mod errors;
pub mod phone;
pub mod pty;
pub mod session;
pub mod telegram;
pub mod voice;
pub mod web;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use config::sessions_persistence;
use tauri::Manager;
use config::settings::SettingsState;
use commands::ac_discovery::DiscoveryBranchWatcher;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::transcript::{TranscriptWriter, MarkerKind};
use session::manager::SessionManager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use voice::tracker::{VoiceTracker, VoiceTrackingState};
use web::auth::WebAccessToken;
use web::broadcast::WsBroadcaster;

/// Returns the full path to the current executable.
pub fn resolve_bin_label() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| config::profile::exe_name().to_string())
}

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

/// Handle to the running web server task, allowing stop control.
pub type WebServerHandle = Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>;

/// Master token generated at app startup. Allows bypassing team validation (can_reach).
/// Ephemeral: lives only in memory, never persisted to disk.
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
        a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// Display value (for printing to stdout at startup only).
    pub fn value(&self) -> &str {
        &self.0
    }
}

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging — stderr + file at config_dir()/app.log
    {
        use std::io::Write;

        // Resolve log file path: <config_dir>/app.log
        let log_file: Option<std::sync::Mutex<std::fs::File>> = config::config_dir()
            .and_then(|dir| {
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join("app.log");
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .ok()
                    .map(|f| {
                        eprintln!("[log] file logging to {}", path.display());
                        std::sync::Mutex::new(f)
                    })
            });
        let log_file = std::sync::Arc::new(log_file);

        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("agentscommander=info")
        )
        .format({
            let log_file = std::sync::Arc::clone(&log_file);
            move |buf, record| {
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let line = format!("{} [{}] {} — {}\n",
                    ts, record.level(), record.target(), record.args());
                // Write to stderr (via env_logger's buf)
                buf.write_all(line.as_bytes())?;
                // Tee to file
                if let Some(ref file_mtx) = *log_file {
                    if let Ok(mut f) = file_mtx.lock() {
                        let _ = f.write_all(line.as_bytes());
                    }
                }
                Ok(())
            }
        })
        .init();
    }

    // Generate master token — printed once to stdout, never persisted
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

    // Create WS broadcaster (shared between Tauri commands and web server)
    let broadcaster = WsBroadcaster::new();

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

    let transcript_writer = TranscriptWriter::new();

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));

    // Idle detector: emits session_idle / session_busy events.
    // Callbacks run on native threads (watcher + PTY read loop).
    // AppHandle.emit() is sync and thread-safe, so no tokio needed.
    // AppHandle is set in setup() via OnceLock; callbacks no-op until then.
    let app_handle_lock: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let handle_for_idle = Arc::clone(&app_handle_lock);
    let handle_for_busy = Arc::clone(&app_handle_lock);
    let transcript_for_idle = transcript_writer.clone();
    let transcript_for_busy = transcript_writer.clone();
    let idle_detector = IdleDetector::new(
        move |id| {
            log::info!("[idle] >>> EMIT session_idle for {}", &id.to_string()[..8]);
            transcript_for_idle.record_marker(id, MarkerKind::Idle);
            if let Some(app) = handle_for_idle.get() {
                let _ = tauri::Emitter::emit(app, "session_idle", serde_json::json!({ "id": id.to_string() }));
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
            transcript_for_busy.record_marker(id, MarkerKind::Busy);
            if let Some(app) = handle_for_busy.get() {
                let _ = tauri::Emitter::emit(app, "session_busy", serde_json::json!({ "id": id.to_string() }));
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
    idle_detector.start();

    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_discovery = Arc::clone(&session_mgr);
    let session_mgr_for_web = Arc::clone(&session_mgr);
    let session_mgr_for_exit = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);
    let broadcaster_for_pty = broadcaster.clone();
    let broadcaster_for_web = broadcaster.clone();
    let web_token_for_server = Arc::clone(&web_access_token);
    let transcript_for_pty = transcript_writer.clone();

    let tg_mgr: TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(TelegramBridgeManager::new(output_senders)));

    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));
    let settings_for_web = Arc::clone(&settings);
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(tg_mgr)
        .manage(voice_tracking)
        .manage(settings)
        .manage(detached_sessions.clone())
        .manage(transcript_writer)
        .manage(web_access_token.clone())
        .manage(broadcaster.clone())
        .manage(WebServerHandle::default())
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start();

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            discovery_branch_watcher.start();
            app.manage(discovery_branch_watcher);

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                git_watcher,
                Some(broadcaster_for_pty),
                transcript_for_pty,
            )));
            app.manage(pty_mgr.clone());

            // Start web server if enabled in settings
            {
                let web_settings = config::settings::load_settings();
                if web_settings.web_server_enabled {
                    let bind = web_settings.web_server_bind.clone();
                    let port = web_settings.web_server_port;
                    println!("[web-token] Remote URL: http://{}:{}/?window=sidebar&remoteToken={}", bind, port, web_access_token.value());

                    let join_handle = web::start_server(
                        bind,
                        port,
                        web_token_for_server,
                        session_mgr_for_web,
                        pty_mgr.clone(),
                        settings_for_web,
                        broadcaster_for_web,
                        app.handle().clone(),
                    );

                    let ws_handle = app.state::<WebServerHandle>();
                    *ws_handle.lock().unwrap() = Some(join_handle);
                }
            }

            // Start the mailbox poller for inter-agent message delivery
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.handle().clone());

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

            // Determine primary monitor size for fallback "SideBar Right" layout
            // Convert to logical pixels (physical / scale) since WebviewWindowBuilder
            // ::inner_size() and ::position() expect logical coordinates
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

            // "SideBar Right" defaults: sidebar on right edge, terminal fills the rest
            let sidebar_w = (screen_w * 0.4).round();
            let default_sidebar = config::settings::WindowGeometry {
                x: primary_x + screen_w - sidebar_w,
                y: primary_y,
                width: sidebar_w,
                height: screen_h,
            };
            let default_terminal = config::settings::WindowGeometry {
                x: primary_x,
                y: primary_y,
                width: screen_w - sidebar_w,
                height: screen_h,
            };

            // Resolve sidebar geometry: saved (physical) → validate → convert to logical → fallback
            let sidebar_geo = match &saved_settings.sidebar_geometry {
                Some(geo) if is_visible_on_monitors(geo, &monitors) => {
                    let logical = physical_to_logical(geo, &monitors);
                    log::info!(
                        "[window-setup] sidebar: saved physical ({}, {}) {}x{} → logical ({}, {}) {}x{}",
                        geo.x, geo.y, geo.width, geo.height,
                        logical.x, logical.y, logical.width, logical.height
                    );
                    logical
                }
                Some(geo) => {
                    log::warn!(
                        "[window-setup] sidebar: saved geometry ({}, {}) {}x{} is OFF-SCREEN, falling back to SideBar Right",
                        geo.x, geo.y, geo.width, geo.height
                    );
                    default_sidebar.clone()
                }
                None => {
                    log::info!("[window-setup] sidebar: no saved geometry, using SideBar Right default");
                    default_sidebar.clone()
                }
            };

            // Resolve terminal geometry: saved (physical) → validate → convert to logical → fallback
            let terminal_geo = match &saved_settings.terminal_geometry {
                Some(geo) if is_visible_on_monitors(geo, &monitors) => {
                    let logical = physical_to_logical(geo, &monitors);
                    log::info!(
                        "[window-setup] terminal: saved physical ({}, {}) {}x{} → logical ({}, {}) {}x{}",
                        geo.x, geo.y, geo.width, geo.height,
                        logical.x, logical.y, logical.width, logical.height
                    );
                    logical
                }
                Some(geo) => {
                    log::warn!(
                        "[window-setup] terminal: saved geometry ({}, {}) {}x{} is OFF-SCREEN, falling back to SideBar Right",
                        geo.x, geo.y, geo.width, geo.height
                    );
                    default_terminal.clone()
                }
                None => {
                    log::info!("[window-setup] terminal: no saved geometry, using SideBar Right default");
                    default_terminal.clone()
                }
            };

            // Create Sidebar window
            let sidebar = WebviewWindowBuilder::new(
                app,
                "sidebar",
                WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title(config::profile::app_title())
            .icon(icon.clone())
            .expect("Failed to set sidebar icon")
            .min_inner_size(200.0, 400.0)
            .decorations(false)
            .zoom_hotkeys_enabled(true)
            .inner_size(sidebar_geo.width, sidebar_geo.height)
            .position(sidebar_geo.x, sidebar_geo.y)
            .build()?;

            // Create Terminal window (hidden until a session is active)
            let terminal = WebviewWindowBuilder::new(
                app,
                "terminal",
                WebviewUrl::App("index.html?window=terminal".into()),
            )
            .title("Terminal")
            .icon(icon)
            .expect("Failed to set terminal icon")
            .min_inner_size(400.0, 300.0)
            .decorations(false)
            .visible(false)
            .zoom_hotkeys_enabled(true)
            .inner_size(terminal_geo.width, terminal_geo.height)
            .position(terminal_geo.x, terminal_geo.y)
            .build()?;

            // Suppress unused variable warnings
            let _ = &sidebar;
            let _ = &terminal;

            // Restore sessions from last run
            let persisted = sessions_persistence::load_sessions();
            if !persisted.is_empty() {
                use tauri::Manager;
                let session_mgr_clone = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>().inner().clone();
                let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                let app_handle = app.handle().clone();

                tauri::async_runtime::spawn(async move {
                    let mut active_id = None;
                    let mut failed_recoverable: Vec<sessions_persistence::PersistedSession> = Vec::new();

                    for ps in &persisted {
                        // Skip sessions whose CWD no longer exists (permanent failure)
                        if !std::path::Path::new(&ps.working_directory).exists() {
                            log::warn!("Skipping restore of '{}': CWD '{}' no longer exists", ps.name, ps.working_directory);
                            continue;
                        }

                        match commands::session::create_session_inner(
                            &app_handle,
                            &session_mgr_clone,
                            &pty_mgr_clone,
                            ps.shell.clone(),
                            ps.shell_args.clone(),
                            ps.working_directory.clone(),
                            Some(ps.name.clone()),
                            None, // No agent_id on restore (auto-detected from shell)
                            None, // No agent label on restore (auto-detected from shell)
                            false, // Persist tooling on restore
                            true,  // This IS a restore
                            ps.git_branch_source.clone(),
                            ps.git_branch_prefix.clone(),
                        ).await {
                            Ok(info) => {
                                if ps.was_active {
                                    active_id = Some(info.id);
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to restore session '{}': {}", ps.name, e);
                                // Preserve for next startup attempt (CWD exists, transient failure)
                                failed_recoverable.push(ps.clone());
                            }
                        }
                    }

                    // Switch to the session that was active when the app closed
                    if let Some(id) = active_id {
                        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                            let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> = session_mgr_clone.read().await;
                            let _ = mgr.switch_session(uuid).await;
                            let _ = tauri::Emitter::emit(&app_handle, "session_switched", serde_json::json!({ "id": id }));
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
                    log::info!("Session restore complete");
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::create_session,
            commands::session::destroy_session,
            commands::session::switch_session,
            commands::session::rename_session,
            commands::session::set_last_prompt,
            commands::session::list_sessions,
            commands::session::get_active_session,
            commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::config::get_settings,
            commands::config::update_settings,
            commands::repos::search_repos,
            commands::telegram::telegram_attach,
            commands::telegram::telegram_detach,
            commands::telegram::telegram_list_bridges,
            commands::telegram::telegram_get_bridge,
            commands::telegram::telegram_send_test,
            commands::window::detach_terminal,
            commands::window::close_detached_terminal,
            commands::window::open_in_explorer,
            commands::window::open_guide_window,
            commands::window::ensure_terminal_window,
            commands::phone::phone_send_message,
            commands::phone::phone_get_inbox,
            commands::phone::phone_list_agents,
            commands::phone::phone_ack_messages,
            commands::voice::voice_transcribe,
            commands::voice::voice_mark_recording,
            commands::voice::voice_had_typing,
            commands::config::save_debug_logs,
            commands::config::open_web_remote,
            commands::config::start_web_server,
            commands::config::stop_web_server,
            commands::config::get_web_server_status,
            commands::config::get_instance_label,
            commands::agent_creator::pick_folder,
            commands::agent_creator::create_agent_folder,
            commands::agent_creator::write_claude_settings_local,
            commands::ac_discovery::discover_ac_agents,
            commands::ac_discovery::check_project_path,
            commands::ac_discovery::create_ac_project,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building application")
        .run({
            let detached_set = detached_sessions.clone();
            move |_app_handle, event| match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Destroyed,
                    ..
                } => {
                    // Cleanup detached terminal tracking
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
                                let mut set = detached_set.lock().unwrap();
                                set.remove(&uuid);
                            }
                        }
                    }
                }
                tauri::RunEvent::Exit => {
                    log::info!("[shutdown] Persisting session state...");
                    let mgr_clone = session_mgr_for_exit.clone();
                    tauri::async_runtime::block_on(async move {
                        let mgr = mgr_clone.read().await;
                        sessions_persistence::persist_current_state(&mgr).await;
                    });
                    log::info!("[shutdown] Session state persisted");
                }
                _ => {}
            }
        });
}
