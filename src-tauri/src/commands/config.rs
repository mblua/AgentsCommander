use std::sync::Arc;
use tauri::State;

use crate::config::settings::{save_settings, load_settings, AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::WebServerHandle;

#[tauri::command]
pub async fn save_debug_logs(content: String) -> Result<(), String> {
    let path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("debug-logs.txt");
    tokio::fs::write(&path, &content)
        .await
        .map_err(|e| format!("Failed to write logs: {}", e))?;
    log::info!("Debug logs saved to {:?} ({} bytes)", path, content.len());
    Ok(())
}

#[tauri::command]
pub async fn get_settings(settings: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let s = settings.read().await;
    let mut result = s.clone();
    result.root_token = None; // never expose root token to frontend
    Ok(result)
}

#[tauri::command]
pub async fn update_settings(
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let mut to_save = new_settings;
    // Preserve existing root token — frontend cannot overwrite it
    to_save.root_token = settings.read().await.root_token.clone();
    save_settings(&to_save)?;
    let mut s = settings.write().await;
    *s = to_save;
    Ok(())
}

#[tauri::command]
pub async fn open_web_remote() -> Result<(), String> {
    let settings = load_settings();
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    let token_path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("web-token.txt");

    let token = std::fs::read_to_string(&token_path)
        .map_err(|e| format!("Cannot read web token: {}", e))?;

    let url = format!(
        "http://{}:{}/?window=browser&remoteToken={}",
        settings.web_server_bind, settings.web_server_port, token.trim()
    );

    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn start_web_server(
    app_handle: tauri::AppHandle,
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
    web_token: State<'_, Arc<WebAccessToken>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    broadcaster: State<'_, WsBroadcaster>,
) -> Result<bool, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    // Check if already listening
    let addr = format!("{}:{}", bind, port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        return Ok(false); // already running
    }

    let join_handle = crate::web::start_server(
        bind,
        port,
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        Arc::clone(&settings),
        (*broadcaster).clone(),
        app_handle,
    );

    *ws_handle.lock().unwrap() = Some(join_handle);
    log::info!("[web-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_web_server(
    ws_handle: State<'_, WebServerHandle>,
) -> Result<bool, String> {
    let mut guard = ws_handle.lock().unwrap();
    if let Some(handle) = guard.take() {
        handle.abort();
        log::info!("[web-server] Stopped via command");
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn get_web_server_status(
    settings: State<'_, SettingsState>,
) -> Result<bool, String> {
    let s = settings.read().await;
    let addr = format!("{}:{}", s.web_server_bind, s.web_server_port);
    drop(s);
    // Probe the port to check if the server is actually listening
    match tokio::net::TcpStream::connect(&addr).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}
