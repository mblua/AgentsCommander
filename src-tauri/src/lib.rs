pub mod commands;
pub mod config;
pub mod errors;
pub mod pty;
pub mod session;
pub mod telegram;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use config::settings::SettingsState;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use session::manager::SessionManager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState, TypingFlagMap};

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let typing_flags: TypingFlagMap = Arc::new(Mutex::new(HashMap::new()));

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

    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(output_senders.clone(), Arc::clone(&idle_detector))));
    let tg_mgr: TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(TelegramBridgeManager::new(output_senders, typing_flags.clone())));

    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));

    tauri::Builder::default()
        .manage(session_mgr)
        .manage(pty_mgr)
        .manage(tg_mgr)
        .manage(typing_flags)
        .manage(settings)
        .manage(detached_sessions.clone())
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
                .expect("Failed to load app icon");

            // Create Sidebar window
            let _sidebar = WebviewWindowBuilder::new(
                app,
                "sidebar",
                WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title("summongate")
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
