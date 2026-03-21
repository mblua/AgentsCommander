pub mod commands;
pub mod config;
pub mod errors;
pub mod pty;
pub mod session;
pub mod telegram;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use config::settings::SettingsState;
use pty::manager::PtyManager;
use session::manager::SessionManager;
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(output_senders.clone())));
    let tg_mgr: TelegramBridgeState =
        Arc::new(tokio::sync::Mutex::new(TelegramBridgeManager::new(output_senders)));

    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(config::settings::load_settings()));

    tauri::Builder::default()
        .manage(session_mgr)
        .manage(pty_mgr)
        .manage(tg_mgr)
        .manage(settings)
        .setup(|app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Create Sidebar window
            let _sidebar = WebviewWindowBuilder::new(
                app,
                "sidebar",
                WebviewUrl::App("index.html?window=sidebar".into()),
            )
            .title("summongate")
            .inner_size(280.0, 600.0)
            .min_inner_size(200.0, 400.0)
            .decorations(false)
            .transparent(false)
            .build()?;

            // Create Terminal window
            let _terminal = WebviewWindowBuilder::new(
                app,
                "terminal",
                WebviewUrl::App("index.html?window=terminal".into()),
            )
            .title("Terminal")
            .inner_size(900.0, 600.0)
            .min_inner_size(400.0, 300.0)
            .decorations(false)
            .build()?;

            Ok(())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running application");
}
