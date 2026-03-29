pub mod cli;
pub mod commands;
pub mod config;
pub mod errors;
pub mod phone;
pub mod pty;
pub mod session;
pub mod telegram;
pub mod voice;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use config::sessions_persistence;
use tauri::Manager;
use config::settings::SettingsState;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::transcript::{TranscriptWriter, MarkerKind};
use session::manager::SessionManager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use voice::tracker::{VoiceTracker, VoiceTrackingState};

/// Returns the full path to the current executable.
pub fn resolve_bin_label() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "agentscommander.exe".to_string())
}

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

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
    // Initialize logging — RUST_LOG defaults to info for our crate
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("agentscommander=info")
    ).init();

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

    println!("[master-token] {}", master_token.value());
    println!("[app-outbox] {}", app_outbox.path());
    log::info!("[master-token] Generated (see stdout)");
    log::info!("[app-outbox] {} (see stdout)", app_outbox.path());

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
                    mgr_clone.read().await.mark_idle(id).await;
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
                    mgr_clone.read().await.mark_busy(id).await;
                });
            }
        },
    );
    idle_detector.start();

    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_exit = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);
    let transcript_for_pty = transcript_writer.clone();

    let tg_mgr: TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(TelegramBridgeManager::new(output_senders)));

    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));

    tauri::Builder::default()
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(tg_mgr)
        .manage(voice_tracking)
        .manage(settings)
        .manage(detached_sessions.clone())
        .manage(transcript_writer)
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start();

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                git_watcher,
                transcript_for_pty,
            )));
            app.manage(pty_mgr);

            // Start the mailbox poller for inter-agent message delivery
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.handle().clone());

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
                .expect("Failed to load app icon");

            // Load saved window geometry
            let saved_settings = config::settings::load_settings();

            // Create Sidebar window
            let mut sidebar_builder = WebviewWindowBuilder::new(
                app,
                "sidebar",
                WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title("Agents Commander")
            .icon(icon.clone())
            .expect("Failed to set sidebar icon")
            .min_inner_size(200.0, 400.0)
            .decorations(false)
            .zoom_hotkeys_enabled(true);

            if let Some(geo) = &saved_settings.sidebar_geometry {
                sidebar_builder = sidebar_builder
                    .inner_size(geo.width, geo.height)
                    .position(geo.x, geo.y);
            } else {
                sidebar_builder = sidebar_builder.inner_size(280.0, 600.0);
            }
            let sidebar = sidebar_builder.build()?;

            // Create Terminal window
            let mut terminal_builder = WebviewWindowBuilder::new(
                app,
                "terminal",
                WebviewUrl::App("index.html?window=terminal".into()),
            )
            .title("Terminal")
            .icon(icon)
            .expect("Failed to set terminal icon")
            .min_inner_size(400.0, 300.0)
            .decorations(false)
            .zoom_hotkeys_enabled(true);

            if let Some(geo) = &saved_settings.terminal_geometry {
                terminal_builder = terminal_builder
                    .inner_size(geo.width, geo.height)
                    .position(geo.x, geo.y);
            } else {
                terminal_builder = terminal_builder.inner_size(900.0, 600.0);
            }
            let terminal = terminal_builder.build()?;

            // Suppress unused variable warnings
            let _ = &sidebar;
            let _ = &terminal;

            // Sync per-agent configs from teams.json so local config.json
            // files stay up to date (team membership, coordinator roles).
            let teams_config = config::dark_factory::load_dark_factory();
            if let Err(e) = config::dark_factory::sync_agent_configs(&teams_config) {
                log::warn!("Failed to sync agent configs on startup: {}", e);
            }

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
            commands::window::open_darkfactory_window,
            commands::window::ensure_terminal_window,
            commands::dark_factory::get_dark_factory,
            commands::dark_factory::save_dark_factory,
            commands::phone::phone_send_message,
            commands::phone::phone_get_inbox,
            commands::phone::phone_list_agents,
            commands::phone::phone_ack_messages,
            commands::voice::voice_transcribe,
            commands::voice::voice_mark_recording,
            commands::voice::voice_had_typing,
            commands::config::save_debug_logs,
            commands::agent_creator::pick_folder,
            commands::agent_creator::create_agent_folder,
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
