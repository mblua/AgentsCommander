pub mod commands;
pub mod config;
pub mod errors;
pub mod pty;
pub mod session;
pub mod telegram;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use config::sessions_persistence;
use tauri::Manager;
use config::settings::SettingsState;
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use session::manager::SessionManager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

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
            if let Some(app) = handle_for_idle.get() {
                let _ = tauri::Emitter::emit(app, "session_idle", serde_json::json!({ "id": id.to_string() }));
            }
        },
        move |id| {
            if let Some(app) = handle_for_busy.get() {
                let _ = tauri::Emitter::emit(app, "session_busy", serde_json::json!({ "id": id.to_string() }));
            }
        },
    );
    idle_detector.start();

    let session_mgr_for_git = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);

    let tg_mgr: TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(TelegramBridgeManager::new(output_senders)));

    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));

    tauri::Builder::default()
        .manage(session_mgr)
        .manage(tg_mgr)
        .manage(settings)
        .manage(detached_sessions.clone())
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
            )));
            app.manage(pty_mgr);

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
                .expect("Failed to load app icon");

            // Create Sidebar window
            let _sidebar = WebviewWindowBuilder::new(
                app,
                "sidebar",
                WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title("Agents Commander")
            .icon(icon.clone())
            .expect("Failed to set sidebar icon")
            .inner_size(280.0, 600.0)
            .min_inner_size(200.0, 400.0)
            .decorations(false)
            .build()?;

            // Create Terminal window
            let _terminal = WebviewWindowBuilder::new(
                app,
                "terminal",
                WebviewUrl::App("index.html?window=terminal".into()),
            )
            .title("Terminal")
            .icon(icon)
            .expect("Failed to set terminal icon")
            .inner_size(900.0, 600.0)
            .min_inner_size(400.0, 300.0)
            .decorations(false)
            .build()?;

            // Restore sessions from last run
            let persisted = sessions_persistence::load_sessions();
            if !persisted.is_empty() {
                use tauri::Manager;
                let session_mgr_clone = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>().inner().clone();
                let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                let app_handle = app.handle().clone();

                tauri::async_runtime::spawn(async move {
                    let mut active_id = None;
                    for ps in &persisted {
                        // Skip sessions whose CWD no longer exists
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
                        ).await {
                            Ok(info) => {
                                if ps.was_active {
                                    active_id = Some(info.id);
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to restore session '{}': {}", ps.name, e);
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

                    // Persist the restored state (new UUIDs)
                    let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> = session_mgr_clone.read().await;
                    sessions_persistence::persist_current_state(&mgr).await;

                    log::info!("Session restore complete");
                });
            }

            Ok(())
        })
        .on_window_event({
            let detached_set = detached_sessions.clone();
            move |window, event| {
                if let tauri::WindowEvent::Destroyed = event {
                    let label = window.label();
                    if label.starts_with("terminal-") {
                        // Extract session id from label: "terminal-<uuid_no_dashes>"
                        let id_no_dashes = &label["terminal-".len()..];
                        // Try to reconstruct UUID from dashless form
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
            }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running application");
}
